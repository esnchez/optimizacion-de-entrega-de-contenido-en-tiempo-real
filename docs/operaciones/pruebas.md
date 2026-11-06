# Pruebas y resultados

Todas las pruebas del proyecto, cómo se ejecutan y qué dieron:

| Tipo | Qué cubre | Dónde |
| --- | --- | --- |
| Unitarias | Comparación de ETag, validación de claves, reciclado de conexiones | módulos de [`content-delivery/src/`](../../content-delivery/src) |
| End-to-end | 18 pruebas del servicio contra una API de S3 real (LocalStack), en Docker Compose, en el host o en el clúster | [`content-delivery/tests/e2e.rs`](../../content-delivery/tests/e2e.rs) |
| Workflow de Temporal | 6 pruebas de `PrescaleForEvent` sin servidor | [`temporal/workflow_test.go`](../../temporal/workflow_test.go) ([temporal](temporal.md#pruebas)) |
| Carga (peticiones/s) | Tráfico mixto y escalado del HPA por tasa de llegada | [`loadtest/k6.js`](../../loadtest/k6.js), perfiles `steady` y `scaling` |
| Conexiones simultáneas | Capacidad de un pod y HPA por conexiones | `loadtest/k6.js`, perfil `connections` |
| Apagado ordenado | Las descargas en curso sobreviven a SIGTERM | [procedimiento manual](#apagado-ordenado) |

Resumen y extrapolación a los objetivos del README: [docs/README](../README.md#resultados-y-extrapolación-a-los-objetivos-del-readme).

## Ejecuciones

### 2026-11-05: k6 fuera del clúster

k6 como contenedor de Docker en la red del nodo de minikube, contra NodePorts temporales
([cómo](#k6-fuera-del-clúster)): k6 deja de gastar la CPU (4) y la memoria (6 GiB) del nodo.
Temporal escalado a 0 durante las pruebas, como en las ejecuciones del 2026-11-01. La edad máxima
de conexión del overlay local (120 s) está activa salvo donde se indica.

| Prueba | Entorno | Resultado |
| --- | --- | --- |
| `task load:capacity:external` (ejecución A, 9 000 conexiones en 6 escalones) | minikube, 1 pod | ✅ 1 247 781 peticiones, 0 errores; p99 del servidor 3 ms a 6 000 y 17 ms a 9 000 conexiones; ~33 KiB por conexión ([resultados](#ejecución-a-con-k6-fuera-del-clúster-2026-11-05)) |
| `task load:scaling:external MAX_CONNECTION_AGE_SECS=0` | minikube, 2-6 pods | ✅ 239 795 peticiones, 0 errores, p99 32 ms; reparto 1 092 / 25 a 3 000 conexiones ([A/B](#ab-de-la-edad-máxima-con-k6-fuera-del-clúster-2026-11-05)) |
| `task load:scaling:external` (120 s) | minikube, 2-6 pods | ✅ 239 881 peticiones, 0 errores, p99 ~22 ms; reparto 511 / 463: **la regresión del 2026-11-01 no se reproduce** ([A/B](#ab-de-la-edad-máxima-con-k6-fuera-del-clúster-2026-11-05)) |

### 2026-11-05: reorganización del repositorio

Tras mover el código a `content-delivery/`, `temporal/`, `k8s/` y `loadtest/` (decisión #48):

| Prueba | Entorno | Resultado |
| --- | --- | --- |
| `task lint` · `task test` | portátil | ✅ sin avisos · 4 / 4 |
| `task temporal:test` | portátil | ✅ 6 / 6 |
| `task e2e` (imagen construida desde `content-delivery/`, `docker-compose.yml` en la raíz) | Docker Compose | ✅ 18 / 18 en 11,9 s |
| `kubectl kustomize` de `k8s/overlays/local`, `k8s/overlays/eks`, `k8s/monitoring`, `k8s/temporal` | portátil | ✅ se generan sin errores |

### 2026-11-01

| Prueba | Entorno | Resultado |
| --- | --- | --- |
| `task e2e` · `task e2e:dev` · `task e2e:cluster` | Compose · host · minikube | ✅ 18 / 18 en los tres modos ([suite](#suite-end-to-end)) |
| `task temporal:test` | portátil | ✅ 6 / 6 |
| Humo de conexiones (2 pods, 1 000 conexiones, 90 s) | minikube | ✅ 0 errores, p99 33 ms ([resultados](#resultados-de-conexiones-simultáneas)) |
| `task load:capacity` (ejecución A) | minikube, 1 pod | ✅ 0 errores; ~38 KiB y ~0.09 milicores por conexión ([resultados](#ejecución-a-capacidad-de-un-pod-2026-11-01)) |
| `task load:scaling` (ejecución B) | minikube, 2-6 pods | ✅ 239 679 peticiones, 0 errores, p99 44 ms; reparto desigual entre pods ([resultados](#ejecución-b-el-hpa-escala-por-conexiones-2026-11-01)) |
| `task load:scaling` con `MAX_CONNECTION_AGE_SECS=120` | minikube, 2-6 pods | ⚠️ reparto ±5 %, 0 errores, pero p99 de cliente 1.1 s (no se reproduce el 2026-11-05) ([resultados](#ejecución-b-con-edad-máxima-de-conexión-2026-11-01)) |

### 2026-10-19

| Prueba | Entorno | Resultado |
| --- | --- | --- |
| e2e (suite de bash anterior) | host · Docker · minikube | ✅ 46/46 · 45/45 · 43/43 |
| k6, objetos pequeños en caché, 2 000 y 4 000 req/s | Docker | ✅ p99 39 y 77 ms, 0 errores ([resultados](#docker-compose-2026-10-19)) |
| Load shedding, apagado ordenado, TLS contra S3 | Docker | ✅ ([resultados](#docker-compose-2026-10-19)) |
| Reinicio progresivo bajo carga, HPA por CPU | minikube | ✅ 0 fallos en 230k peticiones ([resultados](#kubernetes-2026-10-19)) |
| `PROFILE=scaling` a 1 500 req/s, dos ejecuciones | minikube | ✅ 553 834 peticiones, 0 fallidas ([resultados](#escalado-por-tasa-de-llegada-2026-10-19)) |

## Con Task

Todas las pruebas se lanzan con [Task](https://taskfile.dev) desde la raíz del repositorio
([`Taskfile.yml`](../../Taskfile.yml)). En Windows funciona desde PowerShell o Git Bash, con
`minikube`, `kubectl`, `docker`, `helm`, `cargo` y `go` en el PATH.

| Tarea | Prueba | Cuándo |
| --- | --- | --- |
| `task test` | unitarias | Siempre; segundos |
| `task lint` | `cargo fmt --check` + `cargo clippy -D warnings` | Antes de integrar |
| `task e2e` | LocalStack + imagen del servicio en Docker Compose, pruebas e2e y limpieza | Antes de integrar un cambio, en CI. La primera compilación de la imagen tarda ~10 min |
| `task env:up` + `task run` + `task e2e:dev` | e2e contra el servicio en el host ([entorno local](entorno-local.md)) | Día a día: recompilar tarda segundos |
| `task cluster:forward` + `task e2e:cluster` | e2e contra minikube a través del Ingress (port-forwards en otra terminal) | Tras desplegar; ~3.5 min por los TTL reales (60 s y 120 s) |
| `task temporal:test` | workflow de Temporal | Al cambiar `temporal/` |
| `task load:rps` | Job de k6 con `PROFILE=scaling` a través del Ingress (~16 min) | Para ver el HPA por CPU y el reinicio progresivo |
| `task load:capacity` | ejecución A: HPA fijado a 1 réplica, 4 500 conexiones en 3 escalones (~13 min); restaura el HPA aunque falle | Al cambiar recursos o código caliente |
| `task load:scaling` | ejecución B: HPA 2-6 réplicas, 3 000 conexiones en 3 escalones (~13 min) | Al cambiar el HPA |
| `task load:capacity:external` | ejecución A con [k6 fuera del clúster](#k6-fuera-del-clúster): `CONNECTIONS` (9 000) en `STEPS` (6) escalones, ~25 min | Para medir más conexiones de las que caben con k6 en el nodo |
| `task load:scaling:external` | ejecución B con k6 fuera del clúster; `MAX_CONNECTION_AGE_SECS=0` desactiva el reciclado durante la prueba. Exige el HPA en 2/2 réplicas | A/B de la edad máxima de conexión |

## Suite end-to-end

`content-delivery/tests/e2e.rs` es una prueba de integración de Rust: cada comportamiento es una
función `#[tokio::test]` con un nombre que dice qué comprueba. Están marcadas `#[ignore]` para que
`cargo test` solo ejecute las unitarias; Task las lanza con `--ignored --test-threads=1`.

Cada prueba sube sus propios objetos a S3 con una clave única, así que no dependen unas de otras ni
de un bucket preparado. Las llamadas a S3 se cuentan con las métricas del propio servicio
(`cache_requests_total`), enviando las peticiones a una sola instancia (`OPS_URL`).

| Grupo | Pruebas |
| --- | --- |
| Operación | `/healthz`, `/readyz`, `/metrics` responden |
| Objetos pequeños | Cuerpo y metadatos de S3 (`Content-Type`, `Content-Length`, `ETag`, `Last-Modified`, `Cache-Control`, `Accept-Ranges`); `If-None-Match` → 304 con ETag exacto o débil en una lista, 200 con otro; claves con espacios y UTF-8 |
| Objetos grandes | 3 MiB en streaming idénticos byte a byte al objeto de S3; `Range` → 206 con el fragmento exacto y `Content-Range`; `Range` en objetos pequeños; `Range` más allá del final → 416 con `bytes */tamaño` |
| Errores | 404 en GET y HEAD; el 404 se guarda en caché (5 peticiones → 1 `HeadObject`); clave de más de 1024 bytes → 400; ruta desconocida → 404 |
| URL prefirmada | 307 con una URL firmada; 404 si no existe; con `FOLLOW_PRESIGN=1` también la descarga |
| Middleware | `x-request-id` generado y propagado; CORS |
| Caché | 200 peticiones concurrentes a una clave en frío → exactamente 1 `HeadObject` y 1 `GetObject`; un objeto actualizado se sirve tras el TTL |
| Métricas y conexiones | Se exportan las métricas de peticiones, caché y conexiones; `http_connections_open` sube con 10 conexiones abiertas y baja al cerrarlas; una conexión que supera `MAX_CONNECTION_AGE_SECS` recibe `Connection: close` tras una respuesta completa, se cuenta en `http_connections_recycled_total` y el cliente se reconecta solo |

Las variables de entorno de la suite (`BASE_URL`, `OPS_URL`, `RESOLVE`, `S3_ENDPOINT`, `BUCKET`,
`CACHE_TTL_SECS`, `MAX_CONNECTION_AGE_SECS`, `FOLLOW_PRESIGN`) están documentadas al principio de
`e2e.rs` y las fija Task en cada modo. `FOLLOW_PRESIGN` solo vale `1` con el servicio en el host
(`task e2e:dev`): en Docker y en el clúster el servicio firma URLs para el nombre interno de
LocalStack, que no se resuelve desde el host.

**Resultados (2026-11-01):** 18/18 en los tres modos: `task e2e` (dos ejecuciones, ~20 s de
pruebas), `task e2e:dev` (tres ejecuciones, ~12 s) y `task e2e:cluster` (~3 min). Dos pruebas se
ajustaron al ejecutarlas en todos los modos (probar en uno solo no las habría encontrado):

- **Conexiones abiertas** fallaba en el clúster: las sondas de Kubernetes y el scraping de Prometheus
  abren y cierran sus propias conexiones. Ahora abre 10 y admite un margen de 2.
- **Ráfaga de 200 peticiones** hizo 2 llamadas a S3 en Compose: abrir 200 conexiones a través del
  port mapping de Docker Desktop tardó más que el TTL de 2 s, y la entrada caducó a mitad de ráfaga
  (una segunda carga correcta, no un fallo de la caché). Ahora abre las conexiones antes con
  `/healthz` y exige exactamente 1 llamada si la ráfaga cabe en el TTL.

## Prueba de carga (k6)

Un único script, [`loadtest/k6.js`](../../loadtest/k6.js), con tres perfiles (`PROFILE`):

| Perfil | Modelo | Para qué |
| --- | --- | --- |
| `steady` (por defecto) | `VUS` usuarios sin pausa durante `DURATION`; `RANGE_SHARE` de peticiones `Range` | Saturación de una máquina |
| `scaling` | Tasa de llegada que sube de `BASE_RPS` a `PEAK_RPS` y baja (~16 min) | Ver cómo escala y reduce el HPA |
| `connections` | Cada VU es un espectador con una conexión keep-alive: pide un segmento y espera `THINK_TIME` s; `STEPS` escalones hasta `CONNECTIONS` | Capacidad por pod y HPA por conexiones |

En el clúster k6 corre como Job ([`loadtest/k8s/`](../../loadtest/k8s)) y envía sus métricas a
Prometheus por remote write, así que la carga y la reacción del sistema se ven juntas en Grafana
([monitorización](monitorizacion.md#cómo-leer-el-dashboard)). Un port-forward sería el cuello de
botella; por eso k6 va dentro del clúster.

```
 k6 Job ──HTTP──▶ ingress-nginx ──▶ Service ──▶ pods ──▶ LocalStack (S3)   (load:rps)
 k6 Job ──HTTP──▶ Service (L4) ──▶ pods                                   (load:capacity, load:scaling)
   └──remote write──▶ Prometheus ──▶ Grafana "Content Delivery: load, scaling and resources"
```

**Estado del Job:** k6 termina con código distinto de cero cuando falla un umbral (p. ej. p99 > 50 ms),
y entonces el Job acaba **Failed**, no Complete. Por eso las tareas siguen los logs
(`kubectl logs -f job/k6`) en lugar de `kubectl wait --for=condition=complete`, que esperaría para
siempre. Los Jobs terminados se conservan una hora.

### Perfil `scaling` (`task load:rps`)

| Fase | Duración | Carga (tasa de llegada) | Qué observar |
| --- | --- | --- | --- |
| Base | 2 min | 100 req/s | Todo plano. 2 pods, CPU ~7% del request. |
| Rampa | 2 min | 100 → 1 500 req/s | El % de CPU sobre el request sube por encima del objetivo del 60% |
| Pico | 4 min | 1 500 req/s | El HPA añade pods; la utilización vuelve a bajar del objetivo |
| Bajada | 1 min | → 100 req/s | La utilización cae, pero **las réplicas se quedan**: la ventana de reducción de 5 minutos |
| Cola | 7 min | 100 req/s | Unos 5 min después de la bajada, el HPA elimina pods (como mucho un 20% por minuto) |

`PEAK_RPS` y `BASE_RPS` se cambian en el Job. **Tasa de llegada, no usuarios:** con un número fijo
de usuarios que nunca hacen pausa, un sistema más lento recibe automáticamente menos carga, lo que
oculta precisamente el efecto que se quiere ver.

Para probar un reinicio progresivo bajo carga: `task load:rps` en una terminal y
`kubectl -n cdn rollout restart deploy/content-delivery` en otra.

### Perfil `connections` (`task load:capacity`, `task load:scaling`)

El objetivo de 5M del README son conexiones simultáneas. minikube no puede abrir 5M (memoria de k6,
~28 000 puertos efímeros por pod de k6, un nodo de 6 GB), así que el plan es **medir un pod y
extrapolar**. El Job [`k6-connections-job.yaml`](../../loadtest/k8s/k6-connections-job.yaml) pide
`videos/segment.bin` (64 KiB, servido desde memoria) con `THINK_TIME=6` y apunta directamente al
Service, no al Ingress: a través de ingress-nginx los pods solo verían el pool keep-alive de nginx
([kubernetes](kubernetes.md#autoescalado-y-disponibilidad)). Las tareas crean los ConfigMaps del
script y de los parámetros (`CONNECTIONS`, `STEPS`), lanzan el Job y siguen sus logs.

Cálculo a partir de cada escalón de la ejecución A (dashboard: conexiones abiertas, memoria, CPU del
pod, p99 de `/content`, errores):

- memoria por conexión = Δ memoria / Δ conexiones entre escalones;
- CPU por conexión = Δ CPU / Δ conexiones (cada conexión hace 1 / `THINK_TIME` req/s);
- conexiones por pod = el menor de (límite de memoria − base − margen) / memoria por conexión
  y (presupuesto de CPU) / CPU por conexión, con tope donde el p99 supera 50 ms.

Ese número es el `averageValue` del HPA en `k8s/base/hpa.yaml`.

### k6 fuera del clúster

`task load:capacity:external` y `task load:scaling:external` ejecutan el perfil `connections` con
k6 como contenedor de Docker (`k6-external`) en la red `minikube`, contra dos NodePorts temporales
([`loadtest/external/nodeports.yaml`](../../loadtest/external/nodeports.yaml)): el Service del
servicio (`:30080`, sigue siendo balanceo L4 de kube-proxy) y Prometheus (`:30090`, para el remote
write de k6). La tarea los borra al terminar aunque la prueba falle.

Así k6 no compite con el pod por los 4 núcleos y 6 GiB del nodo: con k6 dentro, la ejecución A se
paró en 4 500 conexiones con el nodo lleno. Sí comparte la CPU del portátil (8 hilos, 4 núcleos
físicos): por encima de ~6 000 conexiones el nodo y k6 juntos la saturan
([resultados](#ejecución-a-con-k6-fuera-del-clúster-2026-11-05)). Para cifras de cliente fiables
k6 tiene que ir en otra máquina.

### En Docker Compose

```bash
docker compose --profile app up -d --build --wait
task env:seed
docker run --rm -i --network content-delivery-e2e_default -e BASE_URL=http://app:8080 \
  -e VUS=200 -e DURATION=30s -e RANGE_SHARE=0.2 grafana/k6 run - < loadtest/k6.js
```

La imagen de Compose usa TTL de caché de 2 s (para las pruebas e2e), así que cada entrada caliente
caduca cada 2 s y las peticiones esperan a S3. `RANGE_SHARE` es la fracción de peticiones `Range`,
que siempre van a S3: con LocalStack miden LocalStack. `RANGE_SHARE=0` mide solo el servicio y sus
cachés.

## Apagado ordenado

```bash
# Objeto de 64 MiB, un cliente limitado a 1 MB/s, SIGTERM a los 5 s de empezar la descarga
# (con el entorno de Compose levantado, como en la prueba de carga de arriba)
docker compose exec localstack sh -c \
  'head -c 67108864 /dev/urandom > /tmp/huge.bin && sha256sum /tmp/huge.bin && awslocal s3 cp /tmp/huge.bin s3://content/videos/huge.bin'
docker run -d --name slowclient --network content-delivery-e2e_default curlimages/curl sh -c \
  'curl -s --limit-rate 1M -o /tmp/o http://app:8080/content/videos/huge.bin; sha256sum /tmp/o'
sleep 5; docker compose stop -t 120 app; docker logs -f slowclient   # comparar SHA-256
```

El objeto debe ser mayor que los buffers de socket del kernel (varios MiB). Con objetos pequeños el
cuerpo entero está en los buffers antes de que llegue la señal y la prueba no demuestra nada.

## Resultados de rendimiento

### Docker Compose (2026-10-19)

Portátil, Docker Desktop con una VM de 8 CPU, LocalStack 4.14.

| Prueba | Resultado |
| --- | --- |
| Coalescing, 200 peticiones concurrentes en frío | 1 `HeadObject` + 1 `GetObject` |
| Load shedding (`MAX_CONCURRENT_REQUESTS=4`, 100 concurrentes) | 31 × 206, 69 × 503 (las 69 rechazadas en 2.5 ms en total); `/healthz` 200 en 11–33 ms mientras tanto |
| Apagado ordenado, 64 MiB a 1 MB/s, SIGTERM a mitad de descarga | Completa, el SHA-256 coincide; drenado 49 s + 5 s de vaciado, código de salida 0 |
| TLS contra el S3 real de AWS desde la imagen distroless | Handshake OK (S3 respondió 403 a las credenciales falsas) |
| Tamaño de la imagen | 64.4 MB (distroless `cc-debian13`, uid 65532 no root) |
| k6, objetos pequeños en caché a 2 000 req/s | p50 0.84 ms, p99 39 ms, 0 errores; CPU del servicio ≈ 0.8 cores |
| k6, objetos pequeños en caché a 4 000 req/s | p50 1.4 ms, p99 77 ms, 0 errores. k6 (3.1 cores) saturó la VM y perdió iteraciones |
| k6, mixto con 20% de `Range` | rutas en caché p99 12–16 ms; `Range` p99 ≈ 3 s, limitado por LocalStack al 100% de CPU |
| k6, 200 VUs sin pausa (saturación) | 6 400 req/s, 243k peticiones, 0 errores, 16 MiB de memoria del servicio |

La cifra de 4 000 req/s dice más del portátil (k6 y el servicio comparten una VM) que del servicio.

### Kubernetes (2026-10-19)

minikube con 4 CPU / 6 GB en Docker Desktop.

| Prueba | Resultado |
| --- | --- |
| Despliegue desde cero (hoy `task deploy:local`) | Todos los recursos admitidos (Pod Security `restricted` pasa); pods listos |
| e2e a través del Ingress, 2 pods | **43/43**. Omitidas: descarga prefirmada, recuentos de métricas por pod |
| Rutas de operación a través del Ingress | `/healthz`, `/readyz`, `/metrics` → 404 (no públicas) |
| Coalescing con 2 pods | 200 peticiones concurrentes en frío → 2 HEAD + 2 GET a S3 (uno por pod: las cachés son por pod) |
| **Reinicio progresivo bajo carga** (k6, 50 VUs, 230k peticiones) | **0 peticiones fallidas** |
| HPA por CPU bajo carga | Escaló de 2 → 4 en un solo paso; se mantuvo en 4 con ~54–66% de CPU (dentro de la tolerancia del 10% sobre el objetivo del 60%). 474k peticiones, 0 fallidas |
| Memoria por pod | 11–12 MiB cada uno bajo carga (solo objetos pequeños en caché) |

Las cifras de latencia de estas ejecuciones (p99 ~80 ms a ~3 000 req/s) **no** son la latencia del
servicio: k6, ingress-nginx, LocalStack y los pods comparten las 4 CPU de minikube.

### Escalado por tasa de llegada (2026-10-19)

Dos ejecuciones de `PROFILE=scaling` con `PEAK_RPS=1500`, reconstruidas después desde Prometheus.
Juntas muestran las dos mitades del comportamiento del HPA (entonces por CPU).

**Ejecución 1: escalado hacia arriba desde el mínimo** (resolución de 1 minuto)

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

**Ejecución 2: arranque sobredimensionado** (resolución de 15 s alrededor del pico). Empezó con 4
réplicas, restos de una prueba anterior; en lugar de escalar hacia arriba, el HPA *ajustó el tamaño*:

| Tiempo | Carga ofrecida | % de CPU sobre el request | Deseadas / listas | Notas |
| --- | --- | --- | --- | --- |
| 0–5 min | 100 → 1 500 req/s | 9 → 45% | 4 / 4 | Capacidad de sobra |
| 5 min 30 s | 1 500 req/s | 32–45% | **3** / 4 → 3 | ceil(4 × 45 / 60) = 3. Eliminado **con carga máxima**, sin errores |
| 6–8 min | 1 500 req/s | 54–61% | 3 / 3 | Dentro de la tolerancia: sin cambios |
| 15 min | 100 req/s | ~5% | **2** / 2 | Reducción ~5 min después de la bajada |

553 834 peticiones, **0 fallidas, 0 descartadas**. k6 informó de 4 164 *dropped iterations*: en el
pico el nodo saturado a veces no podía enviar las 1 500 req/s.

En qué se va el tiempo (ejecución 2):

| Medido en | 100 req/s: p50 / p99 | 1 500 req/s: p50 / p99 |
| --- | --- | --- |
| Dentro del servicio (tiempo hasta las cabeceras) | 0.05 / 0.2 ms | 0.05 / 0.9 ms |
| ingress-nginx | — / < 5 ms¹ | — / 90–113 ms |
| Cliente k6 | 1.1 / 2 ms | 1.6–3 / 124–207 ms |

¹ El bucket de latencia más bajo de nginx es de 5 ms.

**CPU en el pico:** el nodo usó 3.2 de sus 4 núcleos: **k6 1.27, ingress-nginx 1.1, el servicio solo
0.43** (≈ 280 µs de CPU por petición). El servicio se mantiene por debajo del milisegundo; la latencia
de cola que ve el cliente es el nodo compartido, sobre todo el generador de carga y el ingress.

### Resultados de conexiones simultáneas

Humo (2026-11-01, 2 pods, 1 000 conexiones, `THINK_TIME=4`, 90 s): 1 000 conexiones mantenidas
(`http_connections_open` = VUs de k6), 26 233 peticiones, 0 fallidas, p99 de segmento 33 ms. El HPA
leyó 501/500 conexiones por pod. Coste para k6: ~0.25 MiB y ~1.6 ms de CPU por petición, lo que fija
los valores del Job (`THINK_TIME=6`).

#### Ejecución A: capacidad de un pod (2026-11-01)

1 pod, `THINK_TIME=6`, segmento de 64 KiB servido desde memoria. Media de cada escalón en régimen
estable (muestras cada 30 s desde Prometheus):

| Conexiones | Memoria del pod | CPU del pod | Peticiones/s | p99 servidor | p99 k6 (cliente) | CPU del nodo* |
| --- | --- | --- | --- | --- | --- | --- |
| 1 500 | 61 MiB | 0.15 cores | 250 | 1.1 ms | 46 ms | 1.2 |
| 3 000 | 116 MiB | 0.26 cores | 498 | 2.1 ms | 76 ms | 1.5 |
| 4 500 | 172 MiB | 0.34 cores | 691 | 20 ms | 3 s | 1.9 (Docker: 4.4 cores, 5.6 de 6 GiB) |

\* Suma de los contenedores según Prometheus; `docker stats` del nodo mide además el sistema y el runtime.

0 errores y 0 peticiones descartadas (503). El escalón de 6 000 no se ejecutó: en el de 4 500
**el nodo de minikube llegó a su límite** y se paró la prueba para no provocar que el kernel matase
pods. El pod usaba solo 0.34 cores: la subida del p99 a 4 500 es del nodo, no del servicio. Por eso
el Job usa ahora 4 500 conexiones en 3 escalones.

**Coste por conexión** (pendiente entre escalones, casi lineal):

- **Memoria: ~38 KiB por conexión** (55.5 MiB por cada 1 500 conexiones, igual en los dos tramos).
- **CPU: ~0.53 ms por petición**, es decir, ~0.09 milicores por conexión con un segmento cada 6 s:
  **~11 000 conexiones por core**.

**Capacidad por pod** (límite de 768 MiB, 256 MiB de caché de cuerpos, ~30 MiB de runtime, 20 % de
margen): (768 − 256 − 30) × 0.8 / 38 KiB ≈ **10 000 conexiones por memoria**. Con la request de CPU
de entonces (250m), el HPA escalaba al 60 % de ella, a **~1 700 conexiones**, mucho antes que el
objetivo provisional de 5 000.

**Aplicado** (`k8s/base`, decisión #42): request de CPU de 1 core y objetivo de **6 000 conexiones
por pod**. A 6 000 conexiones un pod usaría ~0.53 cores (53 % < 60 %, así que la CPU queda como red
de seguridad) y ~500 MiB de 768 MiB incluso con la caché de cuerpos llena.

**Límites de la medida**: sin TLS, un único tamaño de segmento, todo servido desde la caché en
memoria, y extrapolación lineal desde 4 500 conexiones en un nodo compartido con k6 y la monitorización.

#### Ejecución B: el HPA escala por conexiones (2026-11-01)

HPA 2-6 réplicas, objetivo 500 conexiones por pod, 3 000 conexiones en 3 escalones.

| Hora (UTC) | Conexiones | Pods | Conexiones por pod (media) | p99 servidor | p99 k6 |
| --- | --- | --- | --- | --- | --- |
| 13:10–13:12 | 1 000 | 2 | 501 | 0.8 ms | 29–40 ms |
| 13:13:28 | rampa a 2 000 | 2 → 3 | > 500 | 0.9 ms | 33 ms |
| 13:13:58 | | 3 → 4 | | | |
| 13:14–13:17 | 2 000 | 4 | 501 | 0.6–1.5 ms | 17–33 ms |
| 13:17:27 | rampa a 3 000 | 4 → 5 | > 500 | 1.1 ms | 59 ms |
| 13:17:57 | | 5 → 6 | | | |
| 13:18–13:21 | 3 000 | 6 | 501 | 1.6–1.9 ms | 36–45 ms |

239 679 peticiones, 100 % correctas, 0 respuestas 5xx, p99 de segmento 44 ms. Todos los escalados
los provocó la métrica de conexiones (*pods metric http_connections_open above target*), cada uno
unos 30 s después de que la media pasara de 500.

**Reparto desigual entre pods.** La media se mantuvo en el objetivo, pero no el reparto:

| Pod (por antigüedad) | Conexiones a 2 000 (4 pods) | Conexiones a 3 000 (6 pods) |
| --- | --- | --- |
| 1 | 905 | 1 111 |
| 2 | 880 | 1 107 |
| 3 | 186 | 387 |
| 4 | 33 | 267 |
| 5 | — | 120 |
| 6 | — | 14 |

Una conexión se queda en el pod que la aceptó, y kube-proxy reparte cada conexión *nueva* al azar
entre todos los pods. Los dos primeros acabaron con más del doble del objetivo; con el objetivo de
producción (6 000) serían ~13 000 conexiones, por encima de los ~10 000 que permite la memoria. Para
corregirlo se implementó la **edad máxima de conexión** (`MAX_CONNECTION_AGE_SECS`, decisión #43).

**Reducción:** la carga terminó a las 13:22. El HPA esperó su ventana de estabilización de 5 min y
bajó 6 → 4 (13:27:03), 4 → 3 (13:27:53) y 3 → 2 (13:29:00). El primer paso quita 2 pods porque el
20 % de 6 (1,2) se redondea hacia arriba.

#### Ejecución B con edad máxima de conexión (2026-11-01)

14:01–14:15 UTC, `MAX_CONNECTION_AGE_SECS=120`, mismas condiciones. Pod más cargado / menos cargado:

| Momento | Sin edad máxima (13:09) | Con edad máxima de 120 s (14:01) |
| --- | --- | --- |
| 2 000 conexiones, justo tras escalar a 4 pods | 905 / 33 | 836 / 14 |
| 2 000 conexiones, ~2 min después | 905 / 33 (no cambia) | **520 / 484** |
| 3 000 conexiones, ~1 min tras escalar a 6 pods | 1 111 / 14 | **497 / 458** |

- **El reparto funciona:** tras cada escalado los pods quedan a ±5 % del objetivo en 1–2 minutos.
  El HPA siguió escalando por conexiones (2 → 4 → 6).
- **Reciclado:** 10–40 conexiones/s cerradas con `Connection: close`, sin ningún error: 236 403
  peticiones, 100 % correctas.
- **Efecto en la métrica del HPA:** un espectador cuya conexión se ha cerrado no cuenta hasta su
  siguiente segmento (hasta 6 s después). Durante las rachas de reciclado las conexiones abiertas
  quedaron hasta un 10 % por debajo de los espectadores (1 800 de 2 000). El objetivo del HPA debe
  dejar ese margen.
- **Latencia: regresión sin causa identificada.** p99 del cliente de 1.1 s (44 ms sin edad máxima);
  mediana de 1 ms y p95 de 97 ms. La subida se concentra entre las 14:12:30 y las 14:14, en 3 de los
  6 pods (p99 del servidor de ~2 s), con esos pods a solo 0.05 cores. Descartado: presión de CPU o
  memoria del nodo (PSI de node-exporter sin espera de memoria), carga de LocalStack (respuestas en
  ~0.3 s), y la tasa de fallos de caché (igual en las dos ejecuciones y en todos los pods).
  **Repetido el 2026-11-05** con k6 fuera del clúster: no se reproduce, era ruido del nodo
  ([A/B](#ab-de-la-edad-máxima-con-k6-fuera-del-clúster-2026-11-05)).

#### Ejecución A con k6 fuera del clúster (2026-11-05)

22:15–22:41 UTC, 1 pod (request de CPU de 250m, sin límite de CPU, límite de memoria de 768 MiB),
`THINK_TIME=6`, segmento de 64 KiB servido desde memoria, `MAX_CONNECTION_AGE_SECS=120`, 9 000
conexiones en 6 escalones. Media de los 2 últimos minutos de cada escalón (Prometheus cada 15 s;
nodo y k6 con `docker stats`):

| Espectadores (VUs) | Conexiones abiertas | Memoria del pod | CPU del pod | Peticiones/s | p99 servidor | p99 k6 (cliente) | Espera de CPU del pod (PSI) | CPU del nodo / de k6 |
| --- | --- | --- | --- | --- | --- | --- | --- | --- |
| 1 500 | 1 420 | 54 MiB | 0.16 | 250 | 0.6 ms | 17 ms | 1 % | 0.9 / 0.5 |
| 3 000 | 2 845 | 96 MiB | 0.29 | 500 | 0.8 ms | 25 ms | 2 % | 1.1 / 0.7 |
| 4 500 | 4 265 | 141 MiB | 0.46 | 749 | 1.3 ms | 41 ms | 4 % | 1.1 / 0.9 |
| 6 000 | 5 687 | 187 MiB | 0.71 | 997 | 3.1 ms | 96 ms | 8 % | 1.5 / 1.2 |
| 7 500 | 7 105 | 232 MiB | 0.98 | 1 241 | 5.5 ms | 217 ms | 13 % | 2.6 / 2.4 (picos 4.4 / 5.8) |
| 9 000 | 8 515 | 282 MiB | 1.29 | 1 447 | 16.7 ms | 786 ms | 23 % | 2.9 / 3.0 (picos 5.0 / 5.5) |

1 247 781 peticiones, **0 errores** y 0 respuestas 5xx. Las conexiones abiertas quedan un ~5 % por
debajo de los espectadores: con la edad máxima de 120 s activa (no existía en la ejecución A del
2026-11-01), un espectador reciclado no cuenta hasta su siguiente segmento (64–76 reciclados/s a
7 500–9 000).

- **Memoria: ~33 KiB por conexión abierta** (227 MiB entre 1 420 y 8 515), lineal hasta 9 000:
  menos que los ~38 KiB del 2026-11-01. A 9 000 conexiones el pod usa 282 de 768 MiB.
- **CPU: ~0.6 ms por petición hasta 4 500 conexiones** (como el 2026-11-01), pero crece por encima:
  ~1.0 ms entre 4 500 y 6 000 y ~1.3 ms entre 7 500 y 9 000. A la vez sube la espera de CPU del pod
  (8 → 23 %) y la del nodo (27 → 51 %): el nodo y k6 juntos llegan a 6 núcleos de media y a 10 en
  picos de los 8 hilos del portátil. Parte del aumento es contención (hilos compartidos, frecuencia),
  no el servicio; sin hardware dedicado no se ha podido separar.
- **Latencia: el servicio aguanta 9 000 conexiones con p99 de 17 ms** en el servidor, dentro del
  objetivo de 50 ms. El p99 del cliente pasa de 41 ms (4 500) a 96 ms (6 000) y 786 ms (9 000): la
  diferencia es tiempo fuera del pod (k6 sin CPU, red de Docker, kube-proxy), con el portátil saturado.
- **El límite de 4 500 del 2026-11-01 era el nodo**: con k6 fuera, el mismo nodo llevó el pod al
  doble de conexiones sin errores.

#### A/B de la edad máxima con k6 fuera del clúster (2026-11-05)

Mismo escenario que la ejecución B (HPA 2-6, objetivo de 500 por pod, 3 000 conexiones en 3
escalones), cada una empezando con 2 réplicas. Conexiones del pod más cargado / menos cargado:

| Momento | Sin edad máxima (`0`, 23:04 UTC) | Con 120 s (23:25 UTC) |
| --- | --- | --- |
| 1 000 conexiones, 2 pods | 528 / 474 | 503 / 474 |
| 2 000 conexiones, ~1 min tras escalar a 4 pods | 883 / 44 | 805 / 23 |
| 2 000 conexiones, final del escalón | 883 / 44 (no cambia) | **498 / 477** |
| 3 000 conexiones, final del escalón (6 pods) | 1 092 / 25 | **511 / 463** |
| Peticiones · errores | 239 795 · 0 | 239 881 · 0 |
| p95 / p99 del cliente (k6) | 12 / 32 ms | 8.5 / ~22 ms |
| Peor p99 del servidor en 1 min | 2.8 ms | 6.5 ms |
| Conexiones abiertas frente a espectadores a 3 000 | 3 006 | 2 898 (−3.4 %, 14–21 reciclados/s) |

- **Sin edad máxima, el reparto desigual se reproduce** casi igual que el 2026-11-01 (905/33 y
  1 111/14): los pods antiguos se quedan con más del doble del objetivo.
- **Con 120 s, ±5 % del objetivo en cada escalón** y la misma latencia o mejor: la regresión de p99
  1.1 s del 2026-11-01 no aparece. Era el nodo (k6 dentro usaba su CPU), no el reciclado.
- El HPA escaló igual en las dos: 2 → 4 a los ~4.5 min y 4 → 6 a los ~9 min, por la métrica de
  conexiones.

Una primera ejecución con 120 s se descartó porque empezó con 6 réplicas (el HPA, al restaurarse
tras la ejecución A, escaló por la CPU que quedaba); por eso `task load:scaling:external` exige
ahora 2/2 réplicas antes de empezar.

## Hallazgos

1. **Corregido: las descargas se cortaban en el apagado.** axum drenaba las conexiones, pero el
   proceso salía en cuanto se *escribía* el último byte y la cola sin enviar del buffer del kernel
   (~1.2 MiB aquí) se perdía al desaparecer la red del contenedor. Ahora el servicio espera
   `SHUTDOWN_FLUSH_SECS` después de drenar; la prueba de 64 MiB necesitó 54 s de drenado + vaciado,
   más que los 30 s de gracia por defecto de Kubernetes ([el Deployment](kubernetes.md#el-deployment)).
2. **Abierto: bloqueos al caducar la caché.** Cuando termina el TTL de una entrada caliente, las
   siguientes peticiones esperan a S3 (agrupadas, pero esperan). Con TTL de 2 s y un LocalStack
   sobrecargado, el p99 de objetos pequeños llegó a 700 ms; con varios pods los bloqueos se
   multiplican. Lo eliminaría servir la entrada caducada mientras se refresca (stale-while-revalidate),
   que moka no hace de forma nativa.
3. **Por investigar:** ~285 µs de CPU por petición en caché a 4 000 req/s es más de lo esperado.
   Perfilar (p. ej. `cargo flamegraph`) en hardware dedicado antes de optimizar.
4. **En minikube, la latencia de cola mide el nodo.** k6 e ingress-nginx usaban más CPU que el
   servicio; a 4 500 conexiones el nodo, no el pod, era el límite.
5. **k6 reserva todos los VUs al arrancar** (~1.7 GiB con 4 500) y un VU detenido no cierra su
   conexión keep-alive hasta que termina el proceso de k6.
6. **Medir antes de dimensionar.** El objetivo provisional (5 000) y la cifra hipotética de 50 000
   conexiones por pod quedaron sustituidos por 6 000, medido; y la request de CPU resultó decidir qué
   métrica del HPA manda.
7. **Con k6 dentro del nodo, el nodo marcaba el techo.** Sacarlo llevó un pod de 4 500 a 9 000
   conexiones sin errores y deshizo la regresión de latencia de la edad máxima de conexión.
8. **La CPU por petición no es constante con el portátil cargado**: 0.6 ms hasta 4 500 conexiones,
   1–1.3 ms por encima, con la espera de CPU del pod subiendo a la vez. El margen de la request de
   CPU frente al HPA debe calcularse con la cifra alta mientras no se mida en hardware dedicado.
