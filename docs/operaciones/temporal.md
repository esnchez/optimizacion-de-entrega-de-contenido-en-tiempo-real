# Temporal: workflow de pre-escalado

Un workflow sencillo para aprender cómo funciona Temporal y, sobre todo, cómo saber que funciona.
Código en [`src/temporal/`](../../src/temporal), manifiestos en [`src/k8s/temporal/`](../../src/k8s/temporal),
tareas `task temporal:*` en el [`Taskfile.yml`](../../Taskfile.yml).

## El caso de uso: `PrescaleForEvent`

El HPA escala **después** de que llegue la carga: ~30 s para decidir, más el arranque de pods (y de
nodos en producción). Para un evento conocido, como un estreno a las 20:00, interesa tener la
capacidad **antes**. El workflow:

1. Toma un **lease** sobre el HPA y lee su `minReplicas` actual (`AcquirePrescale`). Si otra
   ejecución ya está pre-escalando ese HPA, termina sin tocar nada (resultado `Skipped`).
2. Sube el mínimo, por ejemplo de 2 a 4 (`SetMinReplicas`). El HPA crea los pods al momento.
3. Espera a que los pods estén listos (`WaitForReadyReplicas`). La activity comprueba **una vez** y
   falla si aún no lo están; **Temporal la reintenta** cada 5 s, hasta 2 minutos.
4. Mantiene la capacidad `HoldSeconds` con un **temporizador duradero** (`workflow.Sleep`).
5. Restaura el mínimo original y libera el lease en un solo cambio (`RestoreAndRelease`). Este paso
   va en un `defer` con un contexto desconectado: se ejecuta aunque el workflow falle o se cancele.

### Ejecuciones solapadas: el lease

Sin protección, dos ejecuciones solapadas dejan el mínimo alto para siempre: la segunda lee como
"original" el mínimo ya subido por la primera y, al terminar, restaura ese valor. Un ID de workflow
fijo no basta, porque los Schedules añaden la hora al ID y una ejecución manual podría solaparse con
una programada.

El lock es el propio HPA. `AcquirePrescale` escribe tres anotaciones con concurrencia optimista
(`resourceVersion` en el patch: si dos ejecuciones compiten, una recibe un 409 y, al reintentar, ve
el lease de la otra):

| Anotación | Valor |
| --- | --- |
| `prescale.content-delivery/owner` | `<workflow ID>/<run ID>` de la ejecución dueña |
| `prescale.content-delivery/until` | Caducidad: `HoldSeconds` + 5 min. Si el workflow muere sin liberarlo (p. ej. terminado a la fuerza), caduca solo |
| `prescale.content-delivery/original-min-replicas` | El mínimo a restaurar, visible con `kubectl describe hpa` |

Si otra ejecución tiene un lease vigente, la nueva **termina bien con `Skipped: true`** y no cambia
nada: es un caso esperado, no un fallo, así que no dispara `TemporalWorkflowFailed`. Se cuenta en la
métrica `prescale_skipped_total` y aparece como *skipped* en el panel *Workflow outcomes*.
`RestoreAndRelease` solo restaura si el lease sigue siendo suyo.

Para un estreno diario se lanza con un **Schedule** de Temporal (`task temporal:schedule`, cada día a
las 19:50 UTC).

**Temporal se queda fuera del camino de las peticiones**: el servicio de contenido no sabe que existe.
Temporal es el plano de control (trabajo lento, de varios pasos, que no se puede perder); el servicio
en Rust es el plano de datos ([candidatos](#otros-workflows-candidatos)).

## Conceptos, en este código

| Concepto | Dónde | Qué significa |
| --- | --- | --- |
| **Workflow** | `workflow.go` | Solo decide qué hacer y en qué orden. Debe ser determinista: Temporal lo vuelve a ejecutar (*replay*) a partir del historial para reconstruir su estado, así que nada de llamadas a red, relojes ni aleatorios directamente |
| **Activity** | `activities.go` | El trabajo con efectos (aquí, la API de Kubernetes). Puede fallar y reintentarse; por eso son idempotentes (incluso tomar el lease: si un intento ya lo tomó, el reintento lo reconoce) |
| **RetryPolicy** | `workflow.go` | Cuántas veces y cada cuánto reintentar una activity. Los reintentos no están en el código: los gestiona el servidor |
| **Temporizador duradero** | `workflow.Sleep` | Lo guarda el servidor, no el worker. Si el worker muere, el temporizador sigue y el workflow continúa en otro worker |
| **Historial** | servidor | Cada paso (activity programada, iniciada, completada, temporizador…) queda registrado. Es la fuente de verdad de qué pasó |
| **Task queue** | `operations` | El worker escucha esta cola; los workflows se envían a ella |
| **Worker** | `main.go` | El proceso que ejecuta el código de workflows y activities. Puede haber varios; no guarda estado |
| **Schedule** | `task temporal:schedule` | Lanza el workflow periódicamente (cron), gestionado por el servidor |

## Arquitectura en el clúster

```
                    namespace temporal                                 namespace cdn
 ┌──────────────────────────────────────────────────┐       ┌──────────────────────────────┐
 │ temporal (Deployment)                            │       │ HPA content-delivery         │
 │  `temporal server start-dev`                     │       │ Deployment content-delivery  │
 │  gRPC :7233 · UI :8233 · métricas :9090 · SQLite │       └──────────────▲───────────────┘
 └──────────▲───────────────────────────────────────┘                      │ GET / PATCH
            │ gRPC (tareas de la cola "operations")                       │ (Role: solo ese
 ┌──────────┴───────────────────────────────────────┐                      │  HPA y Deployment)
 │ temporal-worker (Deployment, imagen propia en Go)├──────────────────────┘
 │  métricas :9090                                  │
 └──────────────────────────────────────────────────┘
          ▲ scrape (ServiceMonitor "temporal")
     Prometheus ──► Grafana "Temporal: control-plane workflows" + alertas
```

| Fichero | Qué es |
| --- | --- |
| `src/k8s/temporal/server.yaml` | Servidor de desarrollo de Temporal (`temporalio/temporal:1.9.1`): frontend, history, matching, UI y SQLite en un proceso. SQLite en un `emptyDir`: el historial sobrevive a reinicios del contenedor, no a borrar el pod |
| `src/k8s/temporal/worker.yaml` | El worker (imagen `temporal-worker`, construida desde `src/temporal/Dockerfile`), su ServiceAccount y el Service de métricas |
| `src/k8s/temporal/rbac.yaml` | Role en `cdn`: `get`/`patch` sobre el HPA `content-delivery` (incluidas sus anotaciones de lease) y `get` sobre su Deployment. Nada más |
| `src/k8s/monitoring/servicemonitor-temporal.yaml` | Scraping del servidor y del worker, y las alertas |
| `src/k8s/monitoring/dashboards/temporal.json` | Dashboard de Grafana |

## Cómo se usa

```bash
task temporal:test                     # pruebas del workflow, sin servidor (segundos)
task temporal:deploy                   # servidor + worker en minikube
task temporal:ui                       # UI en http://localhost:8233 (en otra terminal)
task temporal:prescale MIN=4 HOLD=180  # lanza un PrescaleForEvent
task temporal:list                     # workflows recientes y su estado
task temporal:describe ID=prescale-…   # estado e historial de uno
task temporal:schedule                 # Schedule diario a las 19:50 UTC
task temporal:schedule:delete          # y para borrarlo
```

`temporal:deploy` sigue el mismo patrón que `deploy:local`: etiqueta la imagen por su contenido y no
reinicia el worker si no ha cambiado. La CLI de Temporal se usa desde dentro del pod del servidor
(`kubectl exec`), así que no hay que instalarla en el host.

## Cómo saber que funciona

Hay tres niveles, de más detallado a más agregado. Úsalos en este orden cuando algo no cuadre.

### 1. El historial del workflow (UI o CLI): qué pasó exactamente

`task temporal:ui` → http://localhost:8233 → *Workflows*. Cada ejecución muestra su estado
(*Running*, *Completed*, *Failed*, *Canceled*), sus parámetros, su resultado y el historial de eventos
paso a paso. Una activity fallida muestra el error de cada intento (por ejemplo `2 of 4 pods ready
(attempt 1)`) y cuándo será el siguiente reintento. Lo mismo desde la terminal:

```bash
task temporal:describe ID=prescale-20261101-185424
```

Una ejecución correcta (2026-11-01, `MIN=4 HOLD=120`):

```
 7  17:54:28  ActivityTaskCompleted     GetMinReplicas → 2
13  17:54:29  ActivityTaskCompleted     SetMinReplicas 4
19  17:54:35  ActivityTaskCompleted     WaitForReadyReplicas (2.º intento: el 1.º vio 2 de 4 pods listos)
23  17:54:35  TimerStarted              120 s
    17:56:35  TimerFired
30  17:56:35  ActivityTaskCompleted     SetMinReplicas 2 (restauración)
34  17:56:35  WorkflowExecutionCompleted
Result  {"HeldSeconds":120,"OriginalMinReplicas":2,"RaisedTo":4}
```

(Esa ejecución es anterior al lease; hoy el primer y el último paso son `AcquirePrescale` y
`RestoreAndRelease`.)

### 2. Métricas y alertas (Prometheus/Grafana): ¿va bien en general?

El worker publica las métricas del SDK de Temporal en `:9090/metrics`, y el servidor las suyas. En
Grafana, **Dashboards → Temporal: control-plane workflows**:

| Panel | Métrica | Qué mirar |
| --- | --- | --- |
| Workflows completed / failed (1h) | `temporal_workflow_completed_total`, `temporal_workflow_failed_total` | Fallidos > 0: abre la UI y busca el paso que falló |
| Workflow outcomes → *skipped* | `prescale_skipped_total` (métrica propia del workflow) | Ejecuciones que encontraron otra en curso y no cambiaron nada |
| Activity failed attempts (1h) | `temporal_activity_execution_failed_total` | Unos pocos son normales (la espera de pods falla a propósito); muchos indican un problema persistente |
| Worker up | `up{job="temporal-worker"}` | 0: nadie ejecuta los workflows; se quedan esperando en la cola |
| Workflow outcomes | las de arriba, por tipo | Un punto por ejecución terminada |
| Activity attempts and failures | `…_execution_latency_seconds_count`, `…_execution_failed_total` por `activity_type` | Los reintentos de `WaitForReadyReplicas` mientras arrancan los pods |
| Activity duration p95 | `temporal_activity_execution_latency_seconds` | Llamadas a la API de Kubernetes: milisegundos |
| Task schedule-to-start p95 | `temporal_activity_schedule_to_start_latency_seconds` | Tiempo en cola antes de que un worker la recoja: ~0 es sano |
| HPA minimum, desired and ready replicas | `kube_horizontalpodautoscaler_spec_min_replicas`… | **El efecto del workflow**: el mínimo sube, los pods le siguen, el mínimo vuelve. Una anotación marca cada cambio del mínimo |

Las métricas del worker llevan las etiquetas `workflow_type`, `activity_type` y `task_queue`. Su
etiqueta `namespace` (el namespace de Temporal) choca con la del pod y Prometheus la renombra a
`exported_namespace`; por eso las consultas filtran por `job="temporal-worker"`.

Alertas (Prometheus → *Alerts*):

| Alerta | Cuándo |
| --- | --- |
| `TemporalWorkflowFailed` | Algún workflow ha fallado en los últimos 15 min |
| `TemporalWorkerDown` | No hay worker durante 2 min |
| `TemporalActivityRetryingALot` | Más de 20 intentos fallidos de una activity en 10 min (p. ej., un 403 por falta de permisos) |
| `TemporalTaskBacklog` | Las tareas esperan más de 5 s en la cola durante 5 min |

### 3. Logs y el efecto en el sistema

- Logs del worker en JSON: `kubectl -n temporal logs deploy/temporal-worker`. El workflow escribe
  *Raising HPA minimum*, *Pods ready*, *Capacity ready, holding*, *HPA minimum restored*.
- El efecto real: `kubectl -n cdn get hpa content-delivery -w` (el mínimo sube y vuelve).

## Experimentos para entenderlo

Cada uno muestra una propiedad de Temporal. Mira la UI y el dashboard mientras tanto.

1. **Ejecución normal.** `task temporal:prescale MIN=4 HOLD=120`. En la UI, el historial; en Grafana,
   el mínimo del HPA sube a 4 y vuelve a 2.
2. **El temporizador sobrevive al worker.** Lanza con `HOLD=60` y, durante la espera, borra el
   worker: `kubectl -n temporal delete pod -l app.kubernetes.io/name=temporal-worker`. Kubernetes
   crea otro, que retoma el workflow: el temporizador vence a su hora y se restaura el mínimo, sin
   repetir los pasos anteriores. *Probado el 2026-11-01: worker borrado a los 30 s de una espera de
   60 s; `TimerStarted` 17:57:45 → `TimerFired` 17:58:45 y workflow completado.*
3. **Reintentos ante un error persistente.** Quita el permiso `patch` en `rbac.yaml` y aplica, o
   borra el RoleBinding: `kubectl -n cdn delete rolebinding temporal-worker-prescale`. Lanza un
   workflow: `SetMinReplicas` recibe un 403, Temporal la reintenta con espera creciente (2 s, 4 s,
   8 s…) y el error se ve en la UI y en *Activity failed attempts*. Tras 5 intentos el workflow
   falla y salta `TemporalWorkflowFailed`. Vuelve a aplicar con `task temporal:deploy`.
4. **Cancelación con compensación.** Lanza con `HOLD=300` y cancela durante la espera:
   `kubectl -n temporal exec deploy/temporal -- temporal workflow cancel --address 127.0.0.1:7233
   --workflow-id <ID>`. El `defer` restaura el mínimo aunque el workflow termine como *Canceled*.
5. **Ejecuciones solapadas.** Lanza `task temporal:prescale MIN=4 HOLD=90` y, a los pocos segundos,
   `task temporal:prescale MIN=6 HOLD=90`. La segunda termina *Completed* con `Skipped: true` y el
   mínimo se queda en 4; mientras tanto, `kubectl -n cdn describe hpa content-delivery` muestra las
   anotaciones del lease. Al terminar la primera, el mínimo vuelve a 2 y las anotaciones desaparecen.
   *Probado el 2026-11-01: segunda ejecución omitida, mínimo restaurado a 2, `prescale_skipped_total` = 1.*
6. **Sin worker, el trabajo espera.** `kubectl -n temporal scale deploy/temporal-worker --replicas=0`
   y lanza un workflow: queda *Running* sin avanzar (y salta `TemporalWorkerDown`). Al volver a 1
   réplica, continúa.

## Pruebas

`task temporal:test` usa el entorno de pruebas del SDK (`testsuite`): ejecuta el workflow en memoria,
sustituye las activities por mocks y salta los temporizadores. Seis pruebas: el camino normal (sube a
4, espera, restaura a 2), que el lease cubre la espera, que una ejecución que encuentra el lease ocupado
termina sin tocar el HPA, los reintentos hasta que los pods están listos, que el mínimo se restaura
aunque los pods nunca lleguen a estar listos, y la decisión del lease (libre, propio, ajeno vigente,
ajeno caducado, fecha ilegible).

## Otros workflows candidatos

Temporal es el plano de control (trabajo lento, de varios pasos, que no se puede perder si un proceso
muere) y nunca está en el camino de las peticiones. Candidatos, por orden de prioridad:

1. **`PublishContent`**: subida → validación → generación de variantes (renditions, segmentos HLS,
   miniaturas) → escritura en S3 → invalidar o precalentar la CDN → marcar como publicado. Pasos de
   minutos u horas con reintentos: el uso más típico de Temporal. Siguiente paso, en una forma mínima
   (validación → marcar como publicado).
2. **Pre-escalado programado**: hecho, `PrescaleForEvent`. Se eligió primero porque es pequeño, su
   efecto se ve en el dashboard que ya existía y ataca el retraso medido del HPA (~30 s).
3. **Logs a S3** (la caja "Temporal → AWS S3 (Logs)" del diagrama del README): un workflow diario que
   agrega logs de acceso, aciertos de la CDN y ancho de banda en informes de costes.
4. **Ciclo de vida del contenido**: expirar, archivar (S3 Glacier) o borrar contenido.
5. **Pruebas de capacidad automatizadas**: fijar el HPA, lanzar el Job de k6, recoger las métricas y
   restaurar el HPA aunque la prueba falle; lo que hoy hace `task load:capacity`.

## Qué cambia en producción

- **Servidor:** el de desarrollo es un único proceso con SQLite, sin alta disponibilidad. En
  producción, **Temporal Cloud** o el chart de Helm oficial con PostgreSQL o Cassandra, varias
  réplicas de cada servicio y copias de seguridad.
- **Seguridad:** mTLS entre workers y servidor, y autorización por namespace. Aquí no hay ninguna.
- **Monitorización:** las mismas métricas y alertas, enviadas al sistema de monitorización externo
  ([monitorizacion.md](monitorizacion.md#monitorización-en-producción)). Temporal Cloud expone
  también sus propias métricas.
- **Versionado de workflows:** cambiar el código de un workflow con ejecuciones en curso puede romper
  el *replay*. En producción se usan versiones (`workflow.GetVersion` o worker versioning).
- **El pre-escalado en EKS:** subir réplicas no sirve de nada si no hay nodos. El workflow debería
  también preparar nodos (Karpenter) y, si hay CDN, precalentarla.
