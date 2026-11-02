// Workflow PrescaleForEvent: sube el mínimo de réplicas del HPA antes de un evento conocido (un
// estreno), espera a que los pods estén listos, mantiene la capacidad un tiempo y la devuelve a su
// valor original. El HPA por sí solo reacciona *después* de que llegue la carga; esto se adelanta.
//
// Lo que enseña de Temporal:
//   - Workflow vs activities: el workflow solo decide (código determinista); las activities hacen
//     el trabajo con efectos (llamadas a la API de Kubernetes).
//   - Reintentos: WaitForReadyReplicas falla mientras los pods no están listos y Temporal la
//     reintenta según su RetryPolicy, sin bucles en el código.
//   - Temporizador duradero: workflow.Sleep sobrevive a reinicios del worker.
//   - Compensación: el mínimo original se restaura aunque el workflow falle o se cancele.
//   - Exclusión: un lease en el propio HPA impide que dos ejecuciones se solapen.
package main

import (
	"time"

	"go.temporal.io/sdk/temporal"
	"go.temporal.io/sdk/workflow"
)

// TaskQueue es la cola de la que lee el worker y a la que se envían los workflows.
const TaskQueue = "operations"

// Margen del lease sobre el tiempo de espera: cubre la espera de pods (hasta 2 min) y reintentos.
// Si el workflow muere sin liberar el lease (p. ej. terminado a la fuerza), caduca solo.
const leaseMargin = 5 * time.Minute

// PrescaleInput son los parámetros del workflow (JSON al lanzarlo desde la CLI).
type PrescaleInput struct {
	Namespace   string // namespace de Kubernetes del servicio, p. ej. "cdn"
	HPA         string // nombre del HorizontalPodAutoscaler
	Deployment  string // Deployment cuyos pods listos se esperan
	MinReplicas int32  // mínimo de réplicas durante el evento
	HoldSeconds int    // cuánto tiempo mantener la capacidad antes de restaurarla
}

// PrescaleResult queda guardado en el historial del workflow y se ve en la UI.
type PrescaleResult struct {
	OriginalMinReplicas int32
	RaisedTo            int32
	HeldSeconds         int
	// Skipped: otra ejecución ya estaba pre-escalando este HPA y esta no ha cambiado nada.
	Skipped        bool
	SkippedBecause string
}

func PrescaleForEvent(ctx workflow.Context, in PrescaleInput) (PrescaleResult, error) {
	logger := workflow.GetLogger(ctx)
	var a *Activities // solo para referenciar los métodos; el worker registra la instancia real
	info := workflow.GetInfo(ctx)
	owner := info.WorkflowExecution.ID + "/" + info.WorkflowExecution.RunID

	// Llamadas a la API de Kubernetes: rápidas, con reintentos para errores transitorios (incluido
	// el 409 de la concurrencia optimista al tomar el lease).
	ctx = workflow.WithActivityOptions(ctx, workflow.ActivityOptions{
		StartToCloseTimeout: 30 * time.Second,
		RetryPolicy: &temporal.RetryPolicy{
			InitialInterval:    2 * time.Second,
			BackoffCoefficient: 2,
			MaximumInterval:    30 * time.Second,
			MaximumAttempts:    5,
		},
	})

	// 1. Lease: si otra ejecución está pre-escalando este HPA, esta termina sin tocar nada.
	ttl := time.Duration(in.HoldSeconds)*time.Second + leaseMargin
	var lease AcquireResult
	if err := workflow.ExecuteActivity(ctx, a.AcquirePrescale, in.Namespace, in.HPA, owner, ttl).Get(ctx, &lease); err != nil {
		return PrescaleResult{}, err
	}
	if !lease.Acquired {
		logger.Info("Skipping: another prescale is in progress", "heldBy", lease.HeldBy, "until", lease.HeldUntil)
		workflow.GetMetricsHandler(ctx).Counter("prescale_skipped").Inc(1)
		return PrescaleResult{Skipped: true, SkippedBecause: "prescale in progress by " + lease.HeldBy + " until " + lease.HeldUntil}, nil
	}
	original := lease.OriginalMin

	// 2. A partir de aquí, pase lo que pase (error, cancelación), el mínimo vuelve a su valor
	// original y se libera el lease. El contexto desconectado permite ejecutar la activity aunque
	// el workflow se esté cancelando.
	defer func() {
		restoreCtx, cancel := workflow.NewDisconnectedContext(ctx)
		defer cancel()
		err := workflow.ExecuteActivity(restoreCtx, a.RestoreAndRelease, in.Namespace, in.HPA, owner, original).Get(restoreCtx, nil)
		if err != nil {
			logger.Error("Could not restore HPA minimum", "error", err)
		} else {
			logger.Info("HPA minimum restored", "to", original)
		}
	}()

	logger.Info("Raising HPA minimum", "from", original, "to", in.MinReplicas)
	if err := workflow.ExecuteActivity(ctx, a.SetMinReplicas, in.Namespace, in.HPA, in.MinReplicas).Get(ctx, nil); err != nil {
		return PrescaleResult{}, err
	}

	// 3. Espera a que los pods estén listos: la activity falla mientras no lo estén y Temporal la
	// reintenta cada 5 s, hasta 2 minutos. Cada intento fallido se ve en la UI y en las métricas.
	waitCtx := workflow.WithActivityOptions(ctx, workflow.ActivityOptions{
		StartToCloseTimeout: 10 * time.Second,
		RetryPolicy: &temporal.RetryPolicy{
			InitialInterval:    5 * time.Second,
			BackoffCoefficient: 1,
			MaximumAttempts:    24,
		},
	})
	if err := workflow.ExecuteActivity(waitCtx, a.WaitForReadyReplicas, in.Namespace, in.Deployment, in.MinReplicas).Get(waitCtx, nil); err != nil {
		return PrescaleResult{}, err
	}

	// 4. Temporizador duradero: si el worker se reinicia durante la espera, el workflow continúa
	// donde estaba cuando vuelva, sin repetir los pasos anteriores.
	logger.Info("Capacity ready, holding", "seconds", in.HoldSeconds)
	if err := workflow.Sleep(ctx, time.Duration(in.HoldSeconds)*time.Second); err != nil {
		return PrescaleResult{}, err // cancelado durante la espera; el defer restaura
	}

	return PrescaleResult{OriginalMinReplicas: original, RaisedTo: in.MinReplicas, HeldSeconds: in.HoldSeconds}, nil
}
