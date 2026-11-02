# Manual de operaciones

Operación diaria del servicio en Kubernetes (namespace `cdn`). La instalación está en
[kubernetes.md](kubernetes.md).

## Desplegar, revertir, reiniciar

```bash
task deploy:local                                        # build + despliegue (minikube)
kubectl -n cdn rollout status deploy/content-delivery
kubectl -n cdn rollout history deploy/content-delivery
kubectl -n cdn rollout undo deploy/content-delivery      # vuelve a la revisión anterior
kubectl -n cdn rollout restart deploy/content-delivery   # pods nuevos, misma imagen (vacía las cachés)
```

Los rollouts no tienen tiempo de inactividad: un pod nuevo arranca antes de que se detenga uno antiguo,
y los pods antiguos terminan las descargas en curso (verificado: 0 peticiones fallidas de 230k durante un
reinicio con carga).

**Los cambios de configuración** pasan por el generador de ConfigMap de los ficheros kustomization. Un
valor cambiado crea un nombre de ConfigMap nuevo y un rollout. No hagas `kubectl edit` del ConfigMap:
los pods en ejecución no lo recogerían, y el siguiente apply lo sobrescribiría.

## Estado de un vistazo

El dashboard de Grafana "Content Delivery: load, scaling and resources" muestra todo esto en una
pantalla ([monitorizacion.md](monitorizacion.md)). Desde el terminal:

```bash
kubectl -n cdn get pods,hpa
kubectl -n cdn top pods
kubectl -n cdn logs deploy/content-delivery --tail=50          # logs en JSON
kubectl -n cdn port-forward deploy/content-delivery 18080:8080 &
curl -s localhost:18080/metrics | grep -E '^(http_requests_total|http_requests_shed_total|cache_requests_total)'
```

Señales clave (nombres de las métricas en [dependencias.md](../servicio/dependencias.md#observabilidad)):

| Señal | Sano | Si no |
| --- | --- | --- |
| `http_requests_shed_total` aumentando | Debería quedarse en ~0 | Los pods están en `MAX_CONCURRENT_REQUESTS`. Comprueba que el HPA está escalando y si se ha alcanzado `maxReplicas`. |
| Proporción de 5xx en `http_requests_total` | < 0.1% | Consulta la tabla de códigos de estado más abajo |
| p99 de `http_request_duration_seconds` | < 50 ms (rutas en caché) | Cachés frías, latencia de S3 o CPU saturada en el nodo |
| Tasa de aciertos de `cache_requests_total` | Alta para contenido popular | TTL demasiado cortos, o cachés demasiado pequeñas para el conjunto de trabajo |
| Reinicios de pods | 0 | `OOMKilled` → consulta la memoria más abajo |

Estas condiciones también son alertas de Prometheus ([monitorizacion.md](monitorizacion.md#alertas)).

## Qué significa cada código de error

| Código | Causa | Acción |
| --- | --- | --- |
| **503** del servicio | Load shedding (descarte de carga): el pod llegó a `MAX_CONCURRENT_REQUESTS` | Escalar horizontalmente (`maxReplicas` del HPA) o subir el límite junto con el límite de memoria |
| **503** de nginx | No hay pods listos detrás del Service | `kubectl -n cdn get pods`, revisa la readiness |
| **504** | Las cabeceras de respuesta no estaban listas en `REQUEST_TIMEOUT_MS` (S3 lento) | Revisa la latencia de S3 / LocalStack |
| **502** | S3 devolvió un error. Se registra como `storage backend failure` con el error de AWS. | Casi siempre credenciales o permisos. Una clave inexistente sin `s3:ListBucket` vuelve de S3 como **403**, que nosotros devolvemos como 502, no 404 ([configuracion.md](../servicio/configuracion.md)). |
| **416** | El cliente pidió un rango más allá del final del objeto | Problema del cliente |
| **404** para `/healthz`, `/metrics` desde fuera | Intencionado: las rutas de operación no se exponen a través del Ingress | Usa port-forward |

## Ruido conocido en los logs

`WARN ... This checksum is a part-level checksum which can't be validated by the Rust SDK`
aparece con objetos subidos por partes (subidas multipart, p. ej. la AWS CLI para ficheros de más de
~8 MB). La descarga funciona igualmente; el SDK simplemente no puede verificar ese tipo de checksum. Se
podría silenciar desactivando la validación del checksum de respuesta para esas peticiones, pero eso
también eliminaría la validación donde sí funciona, así que de momento se deja como está.

## Memoria

Un pod usa ~12 MiB en reposo o sirviendo objetos pequeños en caché. El peor caso es aproximadamente
`BODY_CACHE_MAX_BYTES` + `MAX_CONCURRENT_REQUESTS` × `STREAM_CHUNK_BYTES` + ~60 MiB, que debe
quedarse por debajo del límite de 768Mi. Si los pods reciben `OOMKilled`, baja uno de esos ajustes o sube el
límite. Cámbialos a la vez.

## Comportamiento al apagarse

Al terminar: preStop 5 s → SIGTERM → `/readyz` devuelve 503 → deja de aceptar conexiones → termina
las respuestas en curso → espera `SHUTDOWN_FLUSH_SECS` (5 s) a que se vacíen los buffers de los sockets → sale.
Todo debe caber en `terminationGracePeriodSeconds` (60 s), o Kubernetes mata el pod y
corta las descargas que queden. Los clientes que descarguen ficheros muy grandes deberían usar `/url/{key}`
(prefirmada, directamente desde S3).

## Depuración dentro de un pod

La imagen es distroless, así que no hay shell. Adjunta un contenedor de depuración efímero:

```bash
kubectl -n cdn debug -it <pod> --image=busybox:1.37 --target=content-delivery
```

## Entorno local

```bash
minikube status
minikube stop          # conserva el clúster; minikube start lo reanuda
minikube delete        # lo elimina por completo
kubectl -n localstack logs deploy/localstack | grep "AWS s3"   # cada llamada a S3 que hace el servicio
```

LocalStack guarda los datos en memoria: después de que se reinicie su pod, el bucket se vuelve a crear solo con
`hello.txt`.

- **Los port-forwards** (`task grafana`, `task temporal:ui`, `task cluster:forward`) se cortan si se
  reinicia el pod de destino o se para minikube: vuelve a lanzar la tarea.
- **Tras `minikube start`**, ejecuta `task monitoring:install`: el reinicio desactiva las métricas de
  ingress-nginx. El historial de Temporal se pierde si se recrea su pod (SQLite en un `emptyDir`).
- **En PowerShell**, `minikube` y `kubectl` (1.37) deben estar en el PATH; si no, `kubectl` resuelve
  al 1.32 de Docker Desktop, cinco versiones por detrás del clúster.
