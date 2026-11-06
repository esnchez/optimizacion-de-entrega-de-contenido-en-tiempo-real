package main

import (
	"context"
	"fmt"
	"strconv"
	"time"

	"go.temporal.io/sdk/activity"
	"go.temporal.io/sdk/temporal"
)

// Activities hace el trabajo con efectos: leer y cambiar objetos de Kubernetes. Son idempotentes
// (repetirlas da el mismo resultado), así que Temporal puede reintentarlas sin riesgo.
type Activities struct {
	Kube *KubeClient
}

// Anotaciones del HPA que hacen de lease: quién está pre-escalando, hasta cuándo y cuál era el
// mínimo original. Evitan que dos ejecuciones solapadas se pisen (la segunda leería como
// "original" el mínimo ya subido por la primera y lo dejaría alto para siempre), vengan de la CLI,
// de un Schedule o de ambos. También dejan el mínimo original a la vista: `kubectl describe hpa`.
const (
	annotationOwner    = "prescale.content-delivery/owner"
	annotationUntil    = "prescale.content-delivery/until"
	annotationOriginal = "prescale.content-delivery/original-min-replicas"
)

type hpaObject struct {
	Metadata struct {
		ResourceVersion string            `json:"resourceVersion"`
		Annotations     map[string]string `json:"annotations"`
	} `json:"metadata"`
	Spec struct {
		MinReplicas int32 `json:"minReplicas"`
	} `json:"spec"`
}

type deploymentObject struct {
	Status struct {
		ReadyReplicas int32 `json:"readyReplicas"`
	} `json:"status"`
}

func hpaPath(namespace, name string) string {
	return fmt.Sprintf("/apis/autoscaling/v2/namespaces/%s/horizontalpodautoscalers/%s", namespace, name)
}

// AcquireResult dice si esta ejecución tiene el lease y, si lo tiene, el mínimo original.
type AcquireResult struct {
	Acquired       bool
	OriginalMin    int32
	HeldBy         string // si no se adquirió: la ejecución que lo tiene
	HeldUntil      string
	AlreadyHolding bool // reintento de una adquisición que ya había tenido éxito
}

// leaseHolder decide, a partir de las anotaciones, si el HPA tiene un lease válido de otra
// ejecución. Sin efectos: se prueba directamente en activities_test.go.
func leaseHolder(annotations map[string]string, owner string, now time.Time) (heldByOther bool, holder, until string) {
	holder, until = annotations[annotationOwner], annotations[annotationUntil]
	if holder == "" || holder == owner {
		return false, holder, until
	}
	expiry, err := time.Parse(time.RFC3339, until)
	if err != nil || now.After(expiry) {
		return false, holder, until // lease caducado o ilegible: se puede tomar
	}
	return true, holder, until
}

// AcquirePrescale toma el lease del HPA durante `ttl` y devuelve su mínimo actual (el que habrá
// que restaurar). Si otra ejecución tiene un lease vigente, devuelve Acquired=false sin cambiar nada.
func (a *Activities) AcquirePrescale(ctx context.Context, namespace, name, owner string, ttl time.Duration) (AcquireResult, error) {
	var hpa hpaObject
	if err := a.Kube.Get(ctx, hpaPath(namespace, name), &hpa); err != nil {
		return AcquireResult{}, err
	}
	ann := hpa.Metadata.Annotations
	if ann[annotationOwner] == owner {
		// Un intento anterior ya tomó el lease (p. ej. el worker murió antes de informar a Temporal).
		original, err := strconv.ParseInt(ann[annotationOriginal], 10, 32)
		if err != nil {
			return AcquireResult{}, fmt.Errorf("lease held by this run but %s is invalid: %w", annotationOriginal, err)
		}
		return AcquireResult{Acquired: true, OriginalMin: int32(original), AlreadyHolding: true}, nil
	}
	if heldByOther, holder, until := leaseHolder(ann, owner, time.Now()); heldByOther {
		activity.GetLogger(ctx).Info("Prescale already in progress", "heldBy", holder, "until", until)
		return AcquireResult{Acquired: false, HeldBy: holder, HeldUntil: until}, nil
	}

	until := time.Now().Add(ttl).UTC().Format(time.RFC3339)
	// resourceVersion en el patch = concurrencia optimista: si otra ejecución cambió el HPA entre el
	// GET y este PATCH, la API responde 409 y Temporal reintenta la activity, que verá el lease nuevo.
	patch := fmt.Sprintf(`{"metadata":{"resourceVersion":%q,"annotations":{%q:%q,%q:%q,%q:"%d"}}}`,
		hpa.Metadata.ResourceVersion,
		annotationOwner, owner, annotationUntil, until, annotationOriginal, hpa.Spec.MinReplicas)
	if err := a.Kube.MergePatch(ctx, hpaPath(namespace, name), patch); err != nil {
		return AcquireResult{}, err
	}
	activity.GetLogger(ctx).Info("Prescale lease acquired", "owner", owner, "until", until, "originalMin", hpa.Spec.MinReplicas)
	return AcquireResult{Acquired: true, OriginalMin: hpa.Spec.MinReplicas}, nil
}

// SetMinReplicas fija el minReplicas del HPA. El HPA sube réplicas al momento si hace falta.
func (a *Activities) SetMinReplicas(ctx context.Context, namespace, name string, minReplicas int32) error {
	activity.GetLogger(ctx).Info("Setting HPA minReplicas", "hpa", namespace+"/"+name, "minReplicas", minReplicas)
	patch := fmt.Sprintf(`{"spec":{"minReplicas":%d}}`, minReplicas)
	return a.Kube.MergePatch(ctx, hpaPath(namespace, name), patch)
}

// RestoreAndRelease devuelve el mínimo original y libera el lease en un solo cambio, solo si esta
// ejecución sigue siendo la dueña. Si el lease ya no es suyo, no toca nada: el mínimo es ahora
// responsabilidad de la otra ejecución.
func (a *Activities) RestoreAndRelease(ctx context.Context, namespace, name, owner string, original int32) error {
	var hpa hpaObject
	if err := a.Kube.Get(ctx, hpaPath(namespace, name), &hpa); err != nil {
		return err
	}
	switch holder := hpa.Metadata.Annotations[annotationOwner]; holder {
	case owner:
	case "":
		// Ya liberado por un intento anterior de esta misma activity.
		activity.GetLogger(ctx).Info("Prescale lease already released")
		return nil
	default:
		// Otra ejecución tomó el lease (el nuestro caducó). Reintentar no cambiaría nada.
		activity.GetLogger(ctx).Warn("Not restoring: lease taken by another run", "heldBy", holder)
		return temporal.NewNonRetryableApplicationError("prescale lease is held by another run: "+holder, "LeaseLost", nil)
	}
	patch := fmt.Sprintf(`{"metadata":{"resourceVersion":%q,"annotations":{%q:null,%q:null,%q:null}},"spec":{"minReplicas":%d}}`,
		hpa.Metadata.ResourceVersion, annotationOwner, annotationUntil, annotationOriginal, original)
	if err := a.Kube.MergePatch(ctx, hpaPath(namespace, name), patch); err != nil {
		return err
	}
	activity.GetLogger(ctx).Info("HPA minimum restored and lease released", "minReplicas", original)
	return nil
}

// WaitForReadyReplicas comprueba una vez si el Deployment tiene al menos `want` pods listos. Si no,
// devuelve un error y Temporal la reintenta según la RetryPolicy del workflow: la espera la gestiona
// Temporal, no un bucle aquí.
func (a *Activities) WaitForReadyReplicas(ctx context.Context, namespace, name string, want int32) error {
	var dep deploymentObject
	path := fmt.Sprintf("/apis/apps/v1/namespaces/%s/deployments/%s", namespace, name)
	if err := a.Kube.Get(ctx, path, &dep); err != nil {
		return err
	}
	attempt := activity.GetInfo(ctx).Attempt
	if dep.Status.ReadyReplicas < want {
		return fmt.Errorf("%d of %d pods ready (attempt %d)", dep.Status.ReadyReplicas, want, attempt)
	}
	activity.GetLogger(ctx).Info("Pods ready", "ready", dep.Status.ReadyReplicas, "attempt", attempt)
	return nil
}
