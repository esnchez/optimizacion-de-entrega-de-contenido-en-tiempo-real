# Kubernetes

El servicio se ejecuta en Kubernetes con manifiestos gestionados por **Kustomize**: una `base` compartida
más un overlay por entorno. En local se ejecuta en **minikube**, con LocalStack proporcionando S3 dentro
del clúster.

## Organización de ficheros

```
src/k8s/
├── base/                       # todo lo común a todos los entornos
│   ├── kustomization.yaml      # namespace, etiquetas, generador de ConfigMap, imagen
│   ├── namespace.yaml          # "cdn", Pod Security "restricted"
│   ├── serviceaccount.yaml     # identidad propia, sin token de la API montado
│   ├── deployment.yaml         # el servicio
│   ├── service.yaml            # ClusterIP :80 -> :8080
│   ├── ingress.yaml            # rutas públicas: solo /content y /url
│   ├── hpa.yaml                # autoescalado por conexiones por pod + CPU, rápido al subir / lento al bajar
│   └── pdb.yaml                # como máximo 1 pod interrumpido a la vez
└── overlays/
    ├── local/                  # minikube
    │   ├── kustomization.yaml  # S3_ENDPOINT, credenciales ficticias, 2-6 réplicas, host cdn.local
    │   └── localstack.yaml     # LocalStack en su propio namespace + script de inicialización del bucket
    └── eks/                    # AWS EKS: ESQUELETO, NO DESPLEGADO (valores marcados con <...>)
        └── kustomization.yaml  # bucket real, IRSA, imagen de ECR
src/k8s/monitoring/              # se aplica por separado, después de kube-prometheus-stack (monitorizacion.md)
├── kube-prometheus-stack-values.yaml
├── prometheus-adapter-values.yaml  # expone http_connections_open al HPA
├── servicemonitor-content-delivery.yaml, ingress-nginx-metrics.yaml, prometheusrule.yaml
└── dashboards/content-delivery.json
```

## Preparación del clúster local

| Herramienta | Versión | Instalada en |
| --- | --- | --- |
| minikube | v1.39.0 | `~/bin/minikube.exe` (binario independiente, SHA-256 verificado; el instalador de winget necesita permisos de administrador) |
| Kubernetes | v1.37.0 | valor por defecto de minikube |
| kubectl | v1.37.0 | `~/bin/kubectl.exe`. El kubectl que incluye Docker Desktop es la 1.32, cinco versiones por detrás; la diferencia de versiones soportada es ±1. |

```bash
minikube start --driver=docker --cpus=4 --memory=6g --addons=ingress,metrics-server
task deploy:local                    # construye la imagen, la carga en minikube, aplica y espera
```

`task deploy:local` ([`Taskfile.yml`](../../Taskfile.yml)):

1. Comprueba que minikube está arrancado y que `kubectl` apunta a él.
2. Construye la imagen (`docker build`). Con la caché de capas tarda segundos si el código no ha
   cambiado.
3. La etiqueta como `dev-<ID de la imagen>` y la carga en minikube, salvo que minikube ya la tenga.
4. Aplica el overlay `local` con esa etiqueta a través de un overlay temporal en `.task/deploy-local/`
   (`images: newTag`, el mecanismo de Kustomize para cambiar la imagen), y espera a LocalStack y al
   servicio.

Una etiqueta única cambia la plantilla del pod, que es lo que dispara un rollout; con una etiqueta
fija (`local`) los pods antiguos seguirían en ejecución. Como la etiqueta sale del contenido de la
imagen, **volver a desplegar sin cambios no reinicia nada**, y `kubectl rollout undo` vuelve a la
imagen anterior.

### Acceder al servicio desde el host

```bash
kubectl -n ingress-nginx port-forward svc/ingress-nginx-controller 8080:80
curl --resolve cdn.local:8080:127.0.0.1 http://cdn.local:8080/content/hello.txt
```

`port-forward` basta para las pruebas funcionales. Para carga, ejecuta k6 **dentro** del clúster
(`loadtest/k8s-job.yaml`), porque un port-forward pasa por un túnel a través del API server y se convierte en el
cuello de botella. (`minikube tunnel` es la alternativa que expone el Ingress en una IP del host).

## Decisiones

### Estructura

| Decisión | Por qué |
| --- | --- |
| **Kustomize base + overlays** (no Helm) | Viene integrado en `kubectl`, YAML plano, sin lenguaje de plantillas. Tenemos un servicio y dos entornos; el empaquetado de Helm todavía no aporta nada. |
| **`configMapGenerator`** para la configuración | El nombre generado incluye un hash del contenido. Cambiar cualquier valor renombra el ConfigMap, lo que cambia la plantilla del pod y dispara una actualización progresiva. Un ConfigMap normal cambiaría sin avisar y los pods mantendrían los valores antiguos hasta que se reiniciasen por casualidad. |
| **Configuración como variables de entorno desde el ConfigMap**, credenciales desde un Secret aparte | Coincide con la configuración 12-factor del servicio ([configuracion.md](../servicio/configuracion.md)). Los secretos quedan fuera del ConfigMap. |
| **Namespace `cdn` dedicado** | Aislamiento, RBAC y cuotas sencillos más adelante, un único sitio donde imponer Pod Security. |
| **minikube** (no kind ni el Kubernetes de Docker Desktop) | Es lo que prevé el README principal, y sus addons proporcionan ingress-nginx y metrics-server (necesario para el HPA) con un solo comando. |

### El Deployment

| Decisión | Por qué |
| --- | --- |
| **Sin campo `replicas`** | El HPA es dueño del número de réplicas. Un campo `replicas` se restablecería en cada `kubectl apply`, peleándose con el HPA. |
| **`maxSurge: 1`, `maxUnavailable: 0`** | Los rollouts añaden un pod nuevo antes de quitar uno antiguo, así que la capacidad nunca baja durante un despliegue. |
| **Requests `cpu: 1`, `memory: 256Mi`; límite `memory: 768Mi`; sin límite de CPU** (en local, request de `cpu: 250m` para que 6 réplicas quepan en minikube) | La request de 1 core hace que el escalado por CPU (60 %, ~6 600 conexiones) llegue después del objetivo de 6 000 conexiones; con 250m llegaba a ~1 700 ([pruebas.md](../pruebas.md#conexiones-simultáneas)). Un límite de CPU hace que la cuota CFS del kernel estrangule el proceso durante el resto de cada periodo de 100 ms una vez agotada. Eso aparece como pausas de decenas de ms en el p99, que es justo lo que queremos evitar. Sin límite, los pods usan la CPU ociosa del nodo durante los picos. La memoria *sí* está limitada, porque la memoria no se puede estrangular, solo matar el proceso. |
| **El presupuesto de memoria coincide con la configuración** | 768Mi ≥ 256Mi de caché de cuerpos (`BODY_CACHE_MAX_BYTES`) + 4000 streams × 64 KiB (`MAX_CONCURRENT_REQUESTS` × `STREAM_CHUNK_BYTES` = 256Mi) + ~60Mi de caché de metadatos y runtime. Medido en reposo: 11–12 MiB por pod. Cambia los tres ajustes a la vez. |
| **Liveness en `/healthz`, readiness en `/readyz`; ninguna comprueba S3** | Si S3 está caído, reiniciar pods no lo arregla y provocaría una tormenta de reinicios. La liveness solo pregunta "¿responde el proceso?". La readiness pasa a 503 al recibir SIGTERM para que el pod salga del balanceador de carga. |
| **`preStop: sleep 5`** (acción sleep nativa) | Cuando un pod empieza a terminar, la retirada del endpoint tarda un momento en llegar a ingress-nginx. La pausa de 5 s mantiene el pod sirviendo hasta que deja de llegar tráfico nuevo. La imagen distroless no tiene el binario `sleep`, así que `exec: [sleep, 5]` fallaría. La acción `sleep` integrada de Kubernetes (≥ 1.30) no necesita nada en la imagen. |
| **`terminationGracePeriodSeconds: 60`** | Presupuesto de apagado: 5 s de preStop + hasta ~50 s drenando descargas en curso + 5 s de `SHUTDOWN_FLUSH_SECS`. Después Kubernetes envía SIGKILL. Las pruebas mostraron que una descarga de 64 MiB a 1 MB/s necesita ~54 s de drenado + vaciado ([pruebas.md](../pruebas.md#hallazgos)); las descargas más largas que el presupuesto deberían usar la ruta prefirmada `/url/{key}`. |
| **Contexto de seguridad**: uid 65532 no root, `readOnlyRootFilesystem`, sin escalada de privilegios, todas las capabilities eliminadas, seccomp `RuntimeDefault` | El servicio no escribe nada en disco y no necesita privilegios. El namespace impone el Pod Security Standard `restricted`, así que un manifiesto que se desvíe de esto se rechaza en la admisión. |
| **`automountServiceAccountToken: false`** | El servicio nunca llama a la API de Kubernetes. Un token montado solo sería superficie de ataque. |
| **Distribución por topología entre hostnames y zonas** (`ScheduleAnyway`) | Perder un nodo o una zona de disponibilidad no debería tumbar todas las réplicas. `ScheduleAnyway` hace que minikube de un solo nodo siga funcionando. |
| **Scrape mediante ServiceMonitor**, no anotaciones | Ver [monitorizacion.md](monitorizacion.md#decisiones). Las anotaciones `prometheus.io/*` se eliminaron cuando se montó la monitorización. |

### Autoescalado y disponibilidad

| Decisión | Por qué |
| --- | --- |
| **CPU al 60% como segunda métrica del HPA** | Red de seguridad si un pod se satura de CPU con pocas conexiones (p. ej. muchas peticiones por conexión) y la métrica que funciona aunque falte Prometheus Adapter. El 60% deja margen mientras arrancan los pods nuevos. |
| **Escalado hacia arriba: sin estabilización, hasta +100% o +4 pods cada 15 s** | El tráfico de contenido llega en picos (un estreno, un contenido viral). Verificado: bajo carga el HPA pasó de 2 a 4 réplicas en un solo paso. |
| **Escalado hacia abajo: 5 min de estabilización, como máximo −20% por minuto** | Evita oscilaciones, y cada pod retirado se lleva su caché caliente. |
| **Base 3–20 réplicas; local 2–6** | Al menos 3 en producción para que un pod pueda estar caído (rollout, drenado de nodo) mientras otros dos siguen sirviendo. |
| **HPA por conexiones abiertas por pod (`http_connections_open`), CPU como red de seguridad** | El objetivo son usuarios simultáneos, y cada usuario mantiene una conexión. La CPU es una señal retrasada para un servicio limitado por E/S, y las peticiones en curso no cuentan a los espectadores que esperan entre segmentos. El HPA calcula las réplicas para cada métrica y usa la mayor. La métrica llega al HPA a través de Prometheus Adapter ([monitorizacion.md](monitorizacion.md#instalación)); sin el adapter el HPA sigue escalando por CPU. Objetivo: 6 000 por pod en la base, medido con la prueba de capacidad (~38 KiB y ~0.09 milicores por conexión, [pruebas.md](../pruebas.md#conexiones-simultáneas)); 500 en local para que el escalado se vea con las pocas miles de conexiones que k6 puede abrir en minikube. |
| **Las conexiones deben llegar directamente a los pods (L4)** | Detrás de ingress-nginx (o de un ALB) los pods solo ven el pool keep-alive del proxy, no a los usuarios, y el propio proxy se convierte en la capa que mantiene 5M de conexiones. Para escalar por conexiones en producción, pon un balanceador de carga L4 (NLB) delante de los pods. |
| **Edad máxima de conexión (`MAX_CONNECTION_AGE_SECS`: 300 s en la base, 120 s en local)** | Una conexión se queda en el pod que la aceptó, y escalar no mueve las existentes: en la ejecución B los pods más antiguos acabaron con 2.2 veces el objetivo ([pruebas.md](../pruebas.md#conexiones-simultáneas)). Pasada la edad máxima, la siguiente respuesta lleva `Connection: close` y el cliente se reconecta a cualquier pod. En local es más corta para que el reparto se vea dentro de los escalones de 3 min de las pruebas. |
| **PodDisruptionBudget `maxUnavailable: 1`** | Un drenado de nodo no puede desalojar más de un pod a la vez. |

### Controlador de Ingress

| Decisión | Por qué |
| --- | --- |
| **Solo se enrutan `/content` y `/url`** | `/healthz`, `/readyz` y `/metrics` se quedan internos: el kubelet y Prometheus llegan a los pods directamente. Exponer las métricas públicamente filtra detalles operativos. Verificado: esas rutas devuelven 404 a través del Ingress. |
| **`proxy-buffering: off`** | nginx reenvía los bytes según llegan en lugar de volcar a disco las respuestas grandes. La memoria se mantiene acotada y la contrapresión de un cliente lento llega al servicio. |
| **`proxy-read-timeout` / `proxy-send-timeout: 60`** | Miden el tiempo *entre* lecturas o escrituras, no la descarga entera, así que los streams largos no tienen problema. Encajan con el presupuesto de terminación de 60 s. |
| **`proxy-body-size: 1m`** | El servicio solo sirve `GET`/`HEAD`; no hay motivo para aceptar cuerpos de petición grandes. |
| **ingress-nginx en local, otra cosa para producción** | El addon de minikube proporciona ingress-nginx, que vale para el trabajo en local. El proyecto Kubernetes anunció la retirada de ingress-nginx (el mantenimiento best-effort terminó en marzo de 2026), así que en EKS es preferible el AWS Load Balancer Controller (ALB) o una implementación de Gateway API. Comprueba el estado actual antes de elegir. |

ingress-nginx pone su propio `X-Request-ID` (32 caracteres hexadecimales) y se lo pasa al servicio,
que lo conserva. Así un mismo ID correlaciona los logs de nginx y del servicio.

### LocalStack en el clúster (solo overlay local)

| Decisión | Por qué |
| --- | --- |
| **Namespace propio `localstack`, Pod Security `baseline`** | LocalStack se ejecuta como root, cosa que `restricted` (en `cdn`) rechaza. Además hace el papel de AWS, que en producción también está fuera de nuestro namespace. |
| **Versión fijada `localstack/localstack:4.14.0`** | Reproducible. La misma major/minor ejecutó todas las pruebas basadas en Docker. Las versiones más recientes de LocalStack usan versionado por calendario y pueden requerir un token de cuenta. |
| **Bucket creado por un script de inicialización** (`/etc/localstack/init/ready.d`), idempotente, montado con `subPath` | El servicio funciona justo después del despliegue. Montar el directorio completo del ConfigMap hacía que LocalStack ejecutase el script dos veces, porque Kubernetes también monta una copia oculta `..data/`. `subPath` monta solo el fichero. |
| **Almacenamiento efímero** | Los objetos se pierden cuando el pod se reinicia y el bucket se vuelve a crear. Aceptable para pruebas, y mantiene la configuración sencilla. |
| **Credenciales ficticias en un Secret** (`test`/`test`) | LocalStack acepta cualquier credencial. En EKS no hay ningún Secret (IRSA). |

### Overlay de EKS (no desplegado)

- **IRSA** (IAM Roles for Service Accounts): el ServiceAccount lleva la anotación de un rol que
  solo permite `s3:GetObject` y `s3:ListBucket` sobre el bucket de contenido. Sin claves estáticas en ningún sitio.
  EKS Pod Identity es una alternativa equivalente.
- La imagen viene de ECR. `S3_ENDPOINT` no está definida, así que el SDK usa el S3 real.
- Pendiente de decidir: ingress (ALB frente a Gateway API), certificados TLS, una CDN CloudFront delante.
- Monitorización: la pila de minikube (Prometheus + Grafana dentro del clúster) no se despliega así en EKS; ver
  [monitorizacion.md](monitorizacion.md#monitorización-en-producción).

## Validar el despliegue

Las pruebas e2e contra el clúster, a través del Ingress ([pruebas.md](../pruebas.md#suite-end-to-end)):

```bash
task cluster:forward   # en otra terminal: Ingress :8080, un pod :18080, LocalStack :4567
task e2e:cluster       # ~3.5 min: usa los TTL reales del overlay local (60 s y 120 s)
```

Prueba de carga y de rollout, con k6 dentro del clúster pasando por el Ingress:

```bash
task load:rps                                             # en una terminal
kubectl -n cdn rollout restart deploy/content-delivery    # en otra, mientras k6 se ejecuta
```

### Resultados (2026-10-19, minikube 4 CPU / 6 GB en Docker Desktop)

| Prueba | Resultado |
| --- | --- |
| Despliegue desde cero con `deploy.sh local` (hoy `task deploy:local`) | Todos los recursos admitidos (Pod Security `restricted` pasa); pods listos |
| e2e a través del Ingress, 2 pods | **43/43**. Omitidas: descarga prefirmada, recuentos de métricas por pod. |
| Rutas de operación a través del Ingress | `/healthz`, `/readyz`, `/metrics` → 404 (no públicas) |
| Coalescing con 2 pods | 200 peticiones concurrentes en frío → 2 HEAD + 2 GET a S3 (uno por pod: las cachés son por pod) |
| **Reinicio progresivo bajo carga** (k6, 50 VUs, 230k peticiones) | **0 peticiones fallidas** |
| HPA bajo carga | Escaló de 2 → 4 en un solo paso; se mantuvo en 4 con ~54–66% de CPU (dentro de la tolerancia del 10% sobre el objetivo del 60%). 474k peticiones, 0 fallidas. |
| Memoria por pod | 11–12 MiB cada uno bajo carga (solo objetos pequeños en caché) |

Las cifras de latencia de estas ejecuciones (p99 ~80 ms a ~3 000 req/s) **no** son la latencia del servicio.
k6, ingress-nginx, LocalStack y los pods comparten las 4 CPU de minikube, así que miden la saturación
del nodo. Los objetivos de latencia necesitan máquinas generadoras de carga separadas.

## Problemas encontrados

- **metrics-server necesita unos minutos tras arrancar el clúster.** Hasta entonces el HPA informa
  `FailedGetResourceMetric` y muestra `cpu: <unknown>`. Es lo esperado.
- **El nodo anuncia 8 CPU pero el contenedor de minikube está limitado a 4.** Con el driver de Docker
  el nodo informa las CPU de la VM de Docker, así que el scheduler puede sobreasignar. Mantén la suma de
  los requests de CPU por debajo de 4.
- **`kubectl rollout status` vuelve cuando los pods nuevos están listos**, mientras que los antiguos pueden seguir
  drenando hasta agotar el periodo de gracia.
- **La descarga de la imagen de LocalStack tarda ~1.5 min** (457 MB) en el primer despliegue.
