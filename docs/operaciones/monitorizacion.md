# Monitorización y observación de las pruebas de carga

Prometheus recoge métricas del servicio, de Kubernetes y del ingress. Grafana las muestra en un
dashboard pensado para responder a una sola pregunta durante una prueba de carga: **¿cómo se
adapta el sistema y cuánto le cuesta?** k6 envía sus propias métricas al mismo Prometheus, así que
la carga que aplicas y la reacción del sistema comparten una misma línea temporal.

```
k6 Job ──remote write──►┐
service pods /metrics ──┤
ingress-nginx :10254 ───┼──► Prometheus ──► Grafana "Content Delivery: load, scaling and resources"
kube-state-metrics ─────┤        │
kubelet / cAdvisor ─────┤        └── alert rules (PrometheusRule)
node-exporter ──────────┘
```

## Instalación

Requisito previo: el clúster local de [kubernetes.md](kubernetes.md).

```bash
task monitoring:install
```

La tarea es idempotente y hace, en orden:

1. Prometheus, Grafana, kube-state-metrics y node-exporter con el chart kube-prometheus-stack
   (versión fijada, `src/k8s/monitoring/kube-prometheus-stack-values.yaml`).
2. Prometheus Adapter, que expone `http_connections_open` al HPA (`custom.metrics.k8s.io`).
3. Nuestros ServiceMonitors, reglas de alerta y dashboard (`kubectl apply -k src/k8s/monitoring`).
4. Las métricas de peticiones del ingress controller de minikube (desactivadas por defecto en el
   addon), solo si no están ya activadas.

Para comprobar que el HPA recibe la métrica de conexiones:

```bash
kubectl get --raw "/apis/custom.metrics.k8s.io/v1beta1/namespaces/cdn/pods/*/http_connections_open"
```

El paso 3 reinicia el ingress controller (unos segundos sin servicio: ocupa el puerto 80 del host,
así que el pod nuevo espera a que termine el antiguo). Volver a ejecutar `minikube addons enable ingress`
lo revierte, **y reiniciar minikube también**: vuelve a ejecutar `task monitoring:install` después de `minikube start`.

El adapter (paso 2) solo publica `http_connections_open`. La CPU y la memoria de `kubectl top`
y la métrica de CPU del HPA siguen viniendo de metrics-server.

Abre las interfaces (deja los terminales abiertos):

```bash
task grafana                                                                        # http://localhost:3000  admin / admin
kubectl -n monitoring port-forward svc/kube-prometheus-stack-prometheus 9090:9090   # http://localhost:9090
```

En Grafana: **Dashboards → Content Delivery: load, scaling and resources**. Se refresca cada
5 s. El chart también instala sus dashboards estándar ("Kubernetes / Compute Resources / …"),
útiles para profundizar en un único pod o nodo. El dashboard **Temporal: control-plane workflows**
cubre el workflow de Temporal ([temporal.md](temporal.md#cómo-saber-que-funciona)).

| Herramienta | Versión | Dónde |
| --- | --- | --- |
| Helm | v3.22.1 | `~/bin/helm.exe` (SHA-256 verificado) |
| kube-prometheus-stack | chart 92.10.1 (operator v0.94.1) | namespace `monitoring` |
| Prometheus | 3.15.0 | |
| Prometheus Adapter | chart 5.3.0 (v0.12.0) | namespace `monitoring` |

## Lanzar una prueba de carga para observarla

```bash
task load:rps          # lanza el Job de k6 y sigue sus logs (resumen de k6 al final)
```

Las pruebas de conexiones simultáneas (`task load:capacity`, `task load:scaling`) están en
[pruebas.md](../pruebas.md#conexiones-simultáneas).

El perfil por defecto, `PROFILE=scaling`, dura unos 16 minutos y está diseñado para que todas
las adaptaciones sean visibles:

| Fase | Duración | Carga (tasa de llegada) | Qué observar |
| --- | --- | --- | --- |
| Base | 2 min | 100 req/s | Todo plano. 2 pods, CPU ~7% del request. |
| Rampa | 2 min | 100 → 1 500 req/s | El % de CPU sobre el request sube por encima de la línea discontinua del objetivo del 60% |
| Pico | 4 min | 1 500 req/s | El HPA añade pods; la utilización vuelve a bajar del objetivo; los pods nuevos asumen su parte |
| Bajada | 1 min | → 100 req/s | La utilización cae, pero **las réplicas se quedan**: la ventana de reducción de 5 minutos |
| Cola | 7 min | 100 req/s | Unos 5 min después de la bajada, el HPA elimina pods (como mucho un 20% por minuto) |

Cambia `PEAK_RPS` / `BASE_RPS` en el Job para apretar más. `PROFILE=steady` (con `VUS`,
`DURATION`) ejecuta usuarios en bucle cerrado sin pausa, lo que es una prueba de saturación más que
una prueba de adaptación.

**Tasa de llegada, no usuarios.** El perfil de escalado usa `ramping-arrival-rate` de k6: envía N
peticiones por segundo sea cual sea el tiempo de respuesta, como el tráfico real. Con un número fijo
de usuarios que nunca hacen pausa, un sistema más lento recibe automáticamente menos carga, lo que
oculta precisamente el efecto que quieres ver.

**Estado del Job:** k6 termina con código distinto de cero cuando falla un umbral (p. ej. p99 > 50 ms),
y entonces el Job acaba **Failed**, no Complete. Síguelo con `kubectl logs -f job/k6`, no con
`kubectl wait --for=condition=complete`, que esperaría para siempre. Los Jobs terminados se conservan
una hora.

## Cómo leer el dashboard

Las filas siguen la cadena de causa y efecto: **carga → resultado → escalado → recursos → caché**.

### Fila de resumen: "¿va bien ahora mismo?"

| Panel | Significado | Sano |
| --- | --- | --- |
| Requests/s | Todas las peticiones respondidas por los pods | Sigue la tasa de k6 |
| p99 /content (server) | Tiempo hasta las cabeceras de respuesta dentro del pod | < 50 ms (rojo por encima) |
| 5xx ratio | Errores de servidor / todas las peticiones | 0 (rojo por encima de 0.1%) |
| Shed (503) /s | Peticiones rechazadas por load shedding (descarte de carga) | 0 |
| Ready pods / HPA desired | Capacidad actual y lo que quiere el HPA | Iguales, salvo brevemente durante el escalado |

### Conexiones: usuarios simultáneos

- **Open connections: offered vs held:** VUs de k6 (una conexión keep-alive por espectador) frente a
  las conexiones TCP abiertas en todos los pods. Si *service* se queda por debajo de *k6 VUs*, se están
  rechazando o cortando conexiones. *ingress* solo tiene sentido cuando la prueba pasa por el Ingress.
- **Open connections per pod vs HPA target:** el HPA mantiene la media en la línea discontinua. Tras
  un escalado hacia arriba los pods nuevos empiezan en cero y solo reciben conexiones *nuevas*, así que
  las líneas convergen despacio; es lo esperado.
- **New and recycled connections/s:** conexiones aceptadas por segundo y conexiones cerradas por
  `MAX_CONNECTION_AGE_SECS` (*recycled*). Durante una rampa, *accepted* sigue a los espectadores
  nuevos. Con espectadores estables, *accepted* ≈ *recycled*: cada conexión reciclada se reconecta
  a cualquier pod, y eso es lo que reparte la carga tras escalar. Si *accepted* está muy por encima
  de *recycled* con carga estable, algo se reconecta en cada petición (sin keep-alive).

### Carga y resultado

- **Requests/s by status (server):** 200 = servida, 304 = revalidación del cliente (sin cuerpo), 206 =
  rango, 503 = descartada. Una franja de 503 creciente significa que los pods llegan a `MAX_CONCURRENT_REQUESTS`.
- **Latency /content: server vs ingress:** los p50/p95/p99 del servidor se miden dentro del pod;
  "ingress p99" lo mide nginx, incluido el salto hasta el pod y la transferencia del cuerpo. **La
  diferencia entre las dos líneas es tiempo pasado fuera del servicio.**
- **k6 VUs and request rate:** la carga ofrecida. En pruebas de tasa de llegada, que suban los VUs
  significa que las respuestas se han ralentizado y k6 ha necesitado más usuarios concurrentes para
  mantener la tasa. Es una señal temprana útil.
- **k6 client latency:** lo que vería un usuario, calculado por ventanas de 1 minuto.

### Escalado: cómo se adapta el sistema

- **HPA replicas:** *desired* se mueve primero, *current* lo sigue y *ready pods* va con el retraso del
  arranque del pod (aquí dentro del mismo minuto: imagen pequeña, sin calentamiento, readiness inmediata). Las líneas discontinuas son mín./máx. Una línea plana en el máximo
  significa que el HPA se ha quedado sin margen (alerta `ContentDeliveryHPAAtMax`).
- **CPU vs HPA target (% of request):** el número sobre el que actúa el HPA. Por encima del objetivo
  discontinuo (más allá de una tolerancia del 10%) escala a `ceil(current × utilization / target)` pods,
  posiblemente varios a la vez.
- **In-flight requests per pod** y **Requests/s per pod:** un pod nuevo empieza en cero y debería
  alcanzar la media en segundos. Un pod atascado en cero no está recibiendo tráfico (readiness, o
  el ingress aún no lo ha detectado).
- **Pods by phase:** `Pending` significa que no hay sitio en el nodo.

### Recursos

- **CPU per pod:** los pods no tienen límite de CPU por diseño, así que pueden superar el request discontinuo.
- **Memory working set per pod:** lo que mira el OOM killer; línea discontinua = límite (768 MiB).
  Crece con los cuerpos en caché y los streams concurrentes.
- **Network out per pod.**
- **CPU by namespace:** quién está usando el nodo. En minikube todo comparte **4 núcleos reales**
  (el nodo *informa* de 8, una peculiaridad del driver de Docker). Si el total apilado se acerca a 4,
  la latencia refleja el nodo, no el servicio.
- **Node memory.**

### Caché y S3

- **Cache hit ratio** baja cuando arrancan pods nuevos (cada uno tiene la caché vacía) y cuando las
  entradas caducan.
- **Cache misses/s** = llamadas a S3 por segundo desde las cachés. Los picos justo después de un escalado
  hacia arriba son los pods nuevos calentándose.
- **LocalStack CPU:** LocalStack es un único proceso de Python. Cerca de 1 núcleo se convierte en el
  cuello de botella de todo lo que llega a S3 (peticiones de rango, objetos grandes, fallos de caché).
- **Container restarts**, **Ingress requests/s by status:** 502/504 aquí pero no en el panel del
  servidor significa que nginx no pudo llegar a un pod.

Los rollouts del servicio aparecen como anotaciones naranjas en todas las series temporales.

## Cómo fueron dos ejecuciones (2026-10-19, minikube 4 CPU)

Ambas ejecuciones usaron `PROFILE=scaling`, `PEAK_RPS=1500`, y se reconstruyeron después a partir de
Prometheus. Juntas muestran las dos mitades del comportamiento del HPA.

### Ejecución 1: escalado hacia arriba desde el mínimo (resolución de 1 minuto)

| Minuto | Carga ofrecida | CPU del servicio (% del request) | Réplicas deseadas / listas | Notas |
| --- | --- | --- | --- | --- |
| 0–2 | 100 req/s | 7% | 2 / 2 | Base |
| 3 | 360 req/s | 19% | 2 / 2 | Empieza la rampa |
| 4 | 1 060 req/s | 52% | 2 / 2 | Aún por debajo del objetivo |
| 5 | 1 480 req/s | **76%** | 2 / 2 | Por encima del 60% + tolerancia |
| 6 | 1 500 req/s | 47% | **3 / 3** | HPA: ceil(2 × 76 / 60) = 3; el pod nuevo estaba listo dentro del mismo minuto |
| 7–8 | 1 500 req/s | 52–55% | 3 / 3 | Estable, por debajo del objetivo |
| 9–10 | → 100 req/s | 4% | 3 / 3 | La carga desaparece; el HPA espera (ventana de estabilización) |
| **15** | 100 req/s | 6% | **2 / 2** | Reducción ~5 min después de la bajada |

### Ejecución 2: arranque sobredimensionado (resolución de 15 segundos alrededor del pico)

La ejecución empezó con **4** réplicas, restos de una prueba corta justo antes. En lugar de
escalar hacia arriba, el HPA *ajustó el tamaño*:

| Tiempo | Carga ofrecida | % de CPU sobre el request | Deseadas / listas | Notas |
| --- | --- | --- | --- | --- |
| 0–5 min | 100 → 1 500 req/s | 9 → 45% | 4 / 4 | Capacidad de sobra; la utilización se queda por debajo del objetivo |
| 5 min 30 s | 1 500 req/s | 32–45% | **3** / 4 → 3 | Caducó la ventana de reducción de la prueba anterior: ceil(4 × 45 / 60) = 3. Eliminado **con carga máxima**, sin errores. |
| 6–8 min | 1 500 req/s | 54–61% | 3 / 3 | Dentro de la tolerancia del 10% sobre el objetivo del 60%: sin cambios |
| 15 min | 100 req/s | ~5% | **2** / 2 | Reducción ~5 min después de la bajada |

553 834 peticiones, **0 fallidas, 0 descartadas**. k6 informó de 4 164 *dropped iterations*: en el pico
el nodo saturado a veces no podía enviar las 1 500 req/s completas. Los picos cortos de VUs en el panel
de k6 (hasta 160–260 VUs) son esos mismos momentos: las respuestas se ralentizaron, así que k6 necesitó
más usuarios concurrentes para mantener la tasa.

### En qué se va el tiempo (ejecución 2)

| Medido en | 100 req/s: p50 / p99 | 1 500 req/s: p50 / p99 |
| --- | --- | --- |
| Dentro del servicio (tiempo hasta las cabeceras) | 0.05 / 0.2 ms | 0.05 / 0.9 ms |
| ingress-nginx | — / < 5 ms¹ | — / 90–113 ms |
| Cliente k6 | 1.1 / 2 ms | 1.6–3 / 124–207 ms |

¹ El bucket de latencia más bajo de nginx es de 5 ms, así que sus cuantiles no pueden marcar menos.

**CPU en el pico:** el nodo entero usó 3.2 de sus 4 núcleos reales: **k6 1.27, ingress-nginx 1.1,
el servicio solo 0.43** (≈ 280 µs de CPU por petición, estable en ambas ejecuciones).

**Conclusión.** En este portátil el propio servicio se mantiene por debajo del milisegundo incluso en el
pico. La latencia de cola que ven los usuarios (p99 > 100 ms) es el nodo compartido y casi saturado, sobre
todo el generador de carga y el ingress. Para medir los límites reales del servicio, ejecuta k6 en hardware
separado. En producción, dimensiona la capacidad del ingress con tanto cuidado como la del servicio.

## Seguimiento desde el terminal

```bash
kubectl -n cdn get hpa content-delivery -w         # réplicas y utilización a medida que cambian
kubectl -n cdn get pods -w                         # pods que aparecen / terminan
kubectl -n cdn describe hpa content-delivery       # Events: cada decisión de escalado con su motivo
kubectl top pods -A --sort-by=cpu                  # quién usa la CPU ahora mismo
```

## PromQL útil

```promql
# Percentiles de latencia del servidor para /content (ventana de 1 min)
histogram_quantile(0.99, sum by (le) (rate(http_request_duration_seconds_bucket{namespace="cdn", path="/content/{*key}"}[1m])))

# Latencia de cliente desde k6 (histograma nativo)
histogram_quantile(0.99, sum(rate(k6_http_req_duration_seconds[1m])))

# Utilización tal como la ve el HPA
100 * sum(rate(container_cpu_usage_seconds_total{namespace="cdn", container="content-delivery"}[1m]))
    / sum(kube_pod_container_resource_requests{namespace="cdn", container="content-delivery", resource="cpu"})

# Microsegundos de CPU por petición (eficiencia; pruebas anteriores: ~270–285 µs)
1e6 * sum(rate(container_cpu_usage_seconds_total{namespace="cdn", container="content-delivery"}[1m]))
    / sum(rate(http_requests_total{namespace="cdn"}[1m]))

# Llamadas a S3 por segundo causadas por fallos de caché
sum by (cache) (rate(cache_requests_total{namespace="cdn", result="miss"}[1m]))
```

## Alertas

Definidas en `src/k8s/monitoring/prometheusrule.yaml`, visibles en http://localhost:9090/alerts.
No hay Alertmanager en local, así que no se envía nada a ningún sitio.

| Alerta | Salta cuando |
| --- | --- |
| `ContentDeliveryP99AboveTarget` | p99 del servidor en `/content` > 50 ms durante 5 min |
| `ContentDeliveryLoadShedding` | Cualquier 503 por load shedding durante 1 min |
| `ContentDeliveryErrorRate` | 5xx > 0.1% durante 5 min |
| `ContentDeliveryHPAAtMax` | Réplicas en `maxReplicas` durante 5 min |
| `ContentDeliveryMemoryNearLimit` | Un pod por encima del 85% de su límite de memoria durante 5 min |
| `ContentDeliveryPodRestarts` | Cualquier reinicio de contenedor en 10 min |

Durante las dos ejecuciones observadas la latencia máxima en el *servidor* se mantuvo por debajo de 1 ms,
así que `ContentDeliveryP99AboveTarget` (que mide el servidor) no saltó aunque fallaran los umbrales de
cliente de k6. Es intencionado: la alerta trata del servicio, y las cifras de cliente en este portátil
están dominadas por el nodo.

## Decisiones

| Decisión | Por qué |
| --- | --- |
| **kube-prometheus-stack con Helm**, versión del chart fijada | Paquete estándar (operator, Prometheus, Grafana, kube-state-metrics, node-exporter, dashboards por defecto). Helm para software de terceros; nuestros manifiestos se quedan en Kustomize. |
| **ServiceMonitor** en lugar de anotaciones `prometheus.io/*` | El operator ignora las anotaciones. El ServiceMonitor hace scrape de todos los pods detrás del Service. Las anotaciones se quitaron del Deployment. |
| **Scrape cada 5 s para el servicio y el ingress**, 15 s para todo lo demás | Detalle al segundo durante las pruebas de carga con poco coste (pocas series por pod). |
| **Selectores sin etiquetas de Helm** (`*SelectorNilUsesHelmValues: false`) | Prometheus recoge nuestros ServiceMonitors y reglas sin etiquetas `release` especiales |
| **Alertmanager desactivado; scrape de etcd, scheduler, controller-manager y kube-proxy desactivado** | Ahorra recursos en local. En minikube esos componentes solo escuchan en localhost y solo generarían alertas de "target down". |
| **node-exporter sin montar el rootfs del host** | Con el driver de Docker `/` no es un montaje compartido y node-exporter no arrancaría |
| **Receptor de remote-write de Prometheus activado; k6 envía métricas** | Carga y reacción en una misma línea temporal. Latencia de k6 como **histogramas nativos**: sus "trend stats" por defecto (p99 como gauge) son acumulativas desde el inicio de la prueba y siguen altas mucho después de que la latencia se recupere. |
| **Buckets de latencia por debajo del milisegundo** (100 µs, 250 µs, 500 µs) añadidos al servicio | Las respuestas en caché tardan ~55 µs en p50. Con 1 ms como bucket más bajo todos los cuantiles marcaban un "1 ms" plano. |
| **Dashboard como código** (JSON en `src/k8s/monitoring/dashboards/`, cargado por el sidecar de Grafana) | Versionado con el servicio; sobrevive a los reinicios de Grafana. Los cambios hechos en la interfaz se pierden: exporta el JSON al repositorio. |
| **Métricas de ingress-nginx activadas parcheando el addon** | Hace falta la latencia en el borde para ver el tiempo pasado fuera del servicio. Un Service + ServiceMonitor hace scrape del puerto 10254. |
| **Sin almacenamiento persistente, 2 días de retención** | Solo en local: las métricas se pierden si se reinicia el pod de Prometheus. En producción el almacenamiento sale del clúster ([monitorización en producción](#monitorización-en-producción)). |

## Coste de la pila de monitorización (en reposo)

| Componente | CPU | Memoria |
| --- | --- | --- |
| Prometheus | ~60m | ~330 MiB |
| Grafana (+2 sidecars) | ~30–110m | ~400 MiB (+150 MiB) |
| operator, kube-state-metrics, node-exporter | ~15m | ~50 MiB |
| Prometheus Adapter | ~35m | ~30 MiB |

Eso son unos 0.2 CPU y 1 GB de los 4 CPU / 6 GB de minikube. Tenlo en cuenta al leer
cifras de saturación.

## Monitorización en producción

En minikube toda la pila vive dentro del clúster: es lo práctico en un portátil y nada depende
de ella. **En producción no se replicaría así.** El patrón habitual separa la recogida de métricas
(dentro del clúster, cerca de lo que se mide) del almacenamiento, las consultas, los dashboards y
las alertas (fuera del clúster).

### Por qué sacar la pila del clúster

1. **El monitor cae con lo monitorizado.** Si el clúster falla (una actualización mala, un nodo
   lleno, un problema de red), un Prometheus y un Grafana dentro de él caen también, justo cuando
   más falta hacen. Es el argumento principal para separarlos.
2. **Compite con el servicio.** En local, Grafana llegó a consumir 2,8 de los 4 cores del nodo
   ([problemas habituales](#problemas-habituales)). En producción eso es capacidad que el servicio
   de contenido no recibe, y una consulta pesada de un dashboard puede afectar a su latencia.
3. **Una sola vista para varios clústeres.** Con EKS en varias regiones o entornos interesa un
   único sitio donde ver y alertar sobre todos. El patrón común es un recolector ligero por
   clúster que envía por remote write a un almacén central (Amazon Managed Prometheus, Mimir o
   Thanos), con las consultas y las alertas evaluadas de forma central.
4. **La recogida sí se queda cerca.** Prometheus (o el agente que lo sustituya) debe estar cerca de
   lo que scrapea; lo que sale del clúster es el almacenamiento y la consulta.

### Arquitectura recomendada en EKS

Basada en la arquitectura de referencia de AWS para EKS:

| Pieza | En producción | ¿Dentro del clúster? |
| --- | --- | --- |
| Recogida de métricas | **Recolector gestionado de AMP** (Amazon Managed Service for Prometheus): sin servidor, multi-AZ, sin agentes que dimensionar ni parchear. Alternativa: Prometheus en modo agente u OpenTelemetry Collector con remote write a AMP. | Gestionado: no. Propio: sí, solo recoge y reenvía |
| Almacenamiento y consultas | AMP (PromQL compatible) | No |
| Dashboards | Amazon Managed Grafana, conectado a AMP con IAM | No |
| Alertas | Reglas de alerta de AMP; las de `prometheusrule.yaml` se trasladan casi sin cambios | No |
| kube-state-metrics, node-exporter | Exportan métricas de Kubernetes y de los nodos | Sí (ligeros) |
| Logs | CloudWatch | Solo el agente |

El JSON del dashboard y las reglas de alerta se reutilizan: AMP usa el mismo lenguaje de consultas.

### Lo que cambia para este proyecto: el HPA depende de la monitorización

El HPA escala por `http_connections_open`, que hoy le llega desde Prometheus a través de
Prometheus Adapter. Con Prometheus fuera del clúster:

- **El escalado por conexiones depende de que AMP responda.** Si AMP no está disponible, el HPA
  sigue escalando solo por CPU. Por eso la CPU se mantiene como segunda métrica del HPA también en
  producción.
- **Prometheus Adapter no firma peticiones con AWS SigV4**, que AMP exige (no verificado aún en
  este proyecto). Opciones: un proxy de firma SigV4 delante de AMP, o sustituir el adapter por
  **KEDA**, cuyo scaler de Prometheus puede consultar AMP con credenciales de AWS. Hay que decidirlo
  antes de EKS ([decisiones.md](decisiones.md#decisiones-abiertas)).

### Vigilar al vigilante

Una alerta que **siempre** está disparada (dead man's switch) y un servicio externo que avisa si deja
de recibirla. Así se detecta que la propia monitorización ha caído, algo que una alerta normal no
puede avisar.

### Fuentes

- [Grafana Labs: ¿debo ejecutar Prometheus en un contenedor?](https://grafana.com/blog/ask-us-anything-should-i-run-prometheus-in-a-container/)
- [AWS: monitorizar EKS con Amazon Managed Grafana](https://docs.aws.amazon.com/grafana/latest/userguide/solution-eks.html)
- [AWS: funcionalidades de Amazon Managed Prometheus](https://aws.amazon.com/prometheus/features/)
- [SREcon24: Prometheus por clúster con remote write a un Mimir central (Grafana Labs)](https://usenix.org/system/files/srecon24americas_slides-smeds.pdf)
- [Multi-clúster con Grafana Mimir](https://oneuptime.com/blog/post/2026-02-09-grafana-mimir-multi-cluster-metrics/view)
- [Comunidad NASSCOM: monitorización de Kubernetes con Prometheus y Grafana](https://community.nasscom.in/communities/web-30/monitoring-kubernetes-clusters-prometheus-and-grafana?page=12)

## Problemas habituales

- **Grafana no responde (503, dashboards vacíos, peticiones de 2 min).** Grafana 13 pasa de
  370 MiB en reposo; con un límite de 512 MiB el recolector de Go entraba en bucle (2,8 cores) y la
  liveness probe lo reiniciaba. Ahora tiene 1 GiB de límite y `GOMEMLIMIT=900MiB`
  (`kube-prometheus-stack-values.yaml`). Cuando Grafana se reinicia, el `port-forward` muere:
  vuelve a lanzarlo.
- **Los ServiceMonitors/reglas nuevos tardan hasta ~1 minuto** en aparecer en Prometheus (reconciliación
  del operator + recarga de la configuración).
- **Volver a ejecutar `minikube addons enable ingress`** elimina el parche `--enable-metrics=true`, y
  los paneles del ingress se quedan vacíos.
- **"No data" en los paneles de k6 en Grafana** es normal cuando no hay ningún Job de k6 en ejecución.
- **Las réplicas que quedan de una prueba anterior** cambian lo que verás: una ejecución que empieza por
  encima del mínimo muestra un ajuste de tamaño (reducción con carga), no un escalado hacia arriba. Para un
  escalado hacia arriba limpio, espera a que el HPA vuelva a `minReplicas` (`kubectl -n cdn get hpa`).
- **Los cuantiles no pueden bajar del bucket más bajo del histograma**: 100 µs para el servicio, 5 ms para
  nginx. Una línea plana exactamente en ese valor significa "como mucho esto".
- **El nodo informa de 8 CPU pero tiene 4** (driver de Docker). Usa el panel "CPU by namespace", no el
  % del nodo, para juzgar la saturación.
