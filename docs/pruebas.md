# Pruebas

| Nivel | Qué | Dónde |
| --- | --- | --- |
| Unitarias | Comparación de ETag, validación de claves, reciclado de conexiones | `task test` |
| End-to-end | 18 pruebas contra una API de S3 real (LocalStack) | [`src/tests/e2e.rs`](../src/tests/e2e.rs), `task e2e` |
| Carga | Tráfico mixto con k6 | [`loadtest/local.js`](../loadtest/local.js) |
| Conexiones simultáneas | Conexiones abiertas por pod, HPA por conexiones | [más abajo](#conexiones-simultáneas) |
| Apagado | Las descargas en curso sobreviven a SIGTERM | procedimiento manual más abajo |
| Kubernetes | e2e a través del Ingress, reinicio progresivo bajo carga, HPA | [kubernetes.md](operaciones/kubernetes.md#validar-el-despliegue) |
| Prueba de carga observada | Perfil de escalado con Grafana | [monitorizacion.md](operaciones/monitorizacion.md#lanzar-una-prueba-de-carga-para-observarla) |

## Cómo se ejecutan

Todas las pruebas se lanzan con [Task](https://taskfile.dev) desde la raíz del repositorio
([`Taskfile.yml`](../Taskfile.yml); `task` lista todas las tareas). En Windows funciona desde PowerShell o
Git Bash, con `minikube`, `kubectl`, `docker`, `helm` y `cargo` en el PATH.

| Comando | Qué hace | Cuándo |
| --- | --- | --- |
| `task test` | Pruebas unitarias | Siempre; segundos |
| `task e2e` | Levanta LocalStack y la imagen del servicio con Docker Compose, ejecuta las pruebas e2e y lo apaga todo | Antes de integrar un cambio, en CI. La primera compilación de la imagen tarda ~10 min |
| `task env:up` + `task run` + `task e2e:dev` | LocalStack en Docker, el servicio en el host con `cargo run` (otra terminal) y las pruebas e2e | Día a día: recompilar tarda segundos |
| `task cluster:forward` + `task e2e:cluster` | Las mismas pruebas contra minikube a través del Ingress | Tras desplegar en el clúster; ~3.5 min por los TTL reales |
| `task lint` | `cargo fmt --check` y clippy | Antes de integrar |
| `task temporal:test` | Pruebas del workflow de Temporal ([temporal.md](operaciones/temporal.md#pruebas)) | Al cambiar `src/temporal` |

## Entorno (Docker Compose)

[`src/tests/compose.yaml`](../src/tests/compose.yaml) define dos servicios:

- **`localstack`** (4.14.0, solo S3) en el puerto 4566, con healthcheck.
- **`app`**: la imagen del servicio construida con `src/Dockerfile`, en el puerto 8089, con TTL de
  caché de 2 s y `MAX_CONNECTION_AGE_SECS=3` para que las pruebas de caducidad y de reciclado duren
  segundos. Va en el perfil `app`: solo la arranca `task e2e`.

No hay paso de inicialización: las pruebas crean el bucket y sus propios objetos. Para las pruebas
de carga y de apagado en Docker, `task env:seed` sube los objetos fijos que usan
(`hello.txt`, `videos/segment.bin`, `videos/big.bin`). `task env:down` lo para y borra todo.

## Suite end-to-end

[`src/tests/e2e.rs`](../src/tests/e2e.rs) es una prueba de integración de Rust: cada comportamiento
es una función `#[tokio::test]` con un nombre que dice qué comprueba, y el resultado aparece en
`cargo test` como cualquier otra prueba. Están marcadas `#[ignore]` para que `cargo test` (sin el
entorno) solo ejecute las unitarias; Task las lanza con `--ignored --test-threads=1`.

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

Variables de entorno (las fija Task en cada modo; documentadas al principio de `e2e.rs`):
`BASE_URL`, `OPS_URL`, `RESOLVE` (p. ej. `cdn.local=127.0.0.1:8080` para el Ingress),
`S3_ENDPOINT`, `BUCKET`, `CACHE_TTL_SECS`, `MAX_CONNECTION_AGE_SECS` (`0` omite la prueba de
reciclado) y `FOLLOW_PRESIGN`.

`FOLLOW_PRESIGN` solo vale `1` con el servicio en el host (`task e2e:dev`): en Docker y en el
clúster el servicio firma URLs para el nombre interno de LocalStack, que no se resuelve desde el host.

**Resultados (2026-11-01):** 18/18 en los tres modos: `task e2e` (Compose, dos ejecuciones, ~20 s
de pruebas), `task e2e:dev` (tres ejecuciones, ~12 s) y `task e2e:cluster` (~3 min). Dos pruebas se
ajustaron al ejecutarlas en todos los modos:

- **Conexiones abiertas** fallaba en el clúster: las sondas de Kubernetes y el scraping de Prometheus
  abren y cierran sus propias conexiones. Ahora abre 10 y admite un margen de 2.
- **Ráfaga de 200 peticiones** hizo 2 llamadas a S3 en Compose: abrir 200 conexiones a través del
  port mapping de Docker Desktop tardó más que el TTL de 2 s, y la entrada caducó a mitad de ráfaga
  (una segunda carga correcta, no un fallo de la caché). Ahora abre las conexiones antes con
  `/healthz` y exige exactamente 1 llamada si la ráfaga cabe en el TTL.

## Prueba de carga

```bash
docker compose -f src/tests/compose.yaml --profile app up -d --build --wait
task env:seed
docker run --rm -i --network content-delivery-e2e_default -e BASE_URL=http://app:8080 \
  -e VUS=200 -e DURATION=30s -e RANGE_SHARE=0.2 grafana/k6 run - < loadtest/local.js
```

La imagen de Compose usa TTL de caché de 2 s (para las pruebas e2e), así que cada entrada caliente
caduca cada 2 s y las peticiones esperan a S3. Para cifras de carga representativas usa el clúster
(`task load:*`, más abajo).

`RANGE_SHARE` es la fracción de peticiones `Range`. Siempre van a S3, así que con LocalStack
miden LocalStack. `RANGE_SHARE=0` mide solo el servicio y sus cachés.

## Apagado ordenado

```bash
# Objeto de 64 MiB, un cliente limitado a 1 MB/s, SIGTERM a los 5 s de empezar la descarga
# (con el entorno de Compose levantado, como en la prueba de carga de arriba)
docker compose -f src/tests/compose.yaml exec localstack sh -c \
  'head -c 67108864 /dev/urandom > /tmp/huge.bin && sha256sum /tmp/huge.bin && awslocal s3 cp /tmp/huge.bin s3://content/videos/huge.bin'
docker run -d --name slowclient --network content-delivery-e2e_default curlimages/curl sh -c \
  'curl -s --limit-rate 1M -o /tmp/o http://app:8080/content/videos/huge.bin; sha256sum /tmp/o'
sleep 5; docker compose -f src/tests/compose.yaml stop -t 120 app; docker logs -f slowclient   # comparar SHA-256
```

El objeto debe ser mayor que los buffers de socket del kernel (varios MiB). Con objetos pequeños el
cuerpo entero está en los buffers antes de que llegue la señal y la prueba no demuestra nada.

## Conexiones simultáneas

El objetivo de 5M del README son conexiones simultáneas: espectadores que mantienen una conexión abierta y piden
un segmento de vídeo cada pocos segundos. minikube no puede abrir 5M (memoria de k6, ~28 000 puertos efímeros por
pod de k6, un nodo de 6 GB), así que el plan es **medir un pod y extrapolar**.

`PROFILE=connections` en [`loadtest/local.js`](../loadtest/local.js): cada VU de k6 es un espectador con
una conexión keep-alive. Pide `videos/segment.bin` (64 KiB, servido desde memoria) y espera
`THINK_TIME` s (4 por defecto en el script, 6 en el Job). La carga sube en `STEPS` escalones hasta `CONNECTIONS`, manteniendo cada escalón durante
`STEP_HOLD`, así que cada escalón da un punto de datos. El Job
[`loadtest/k8s-job-connections.yaml`](../loadtest/k8s-job-connections.yaml) apunta directamente al Service,
no al Ingress: a través de ingress-nginx los pods solo verían el pool keep-alive de nginx
([kubernetes.md](operaciones/kubernetes.md#autoescalado-y-disponibilidad)).

### Ejecución A: capacidad de un pod

```bash
task load:capacity    # fija el HPA a 1 réplica, 4 500 conexiones en 3 escalones (~13 min), y lo restaura
```

La tarea crea los ConfigMaps del script y de los parámetros (`CONNECTIONS`, `STEPS`), lanza el Job,
sigue sus logs, y vuelve a poner el HPA en 2-6 réplicas aunque la prueba falle o se interrumpa.

Para cada escalón, lee en el dashboard: conexiones abiertas, memoria (working set), CPU del pod, p99
de `/content`, errores. Después:

- memoria por conexión = Δ memoria / Δ conexiones entre escalones;
- CPU por conexión = Δ CPU / Δ conexiones (cada conexión hace 1 / `THINK_TIME` req/s);
- conexiones por pod = el menor de (límite de memoria − base − margen) / memoria por conexión
  y (presupuesto de CPU) / CPU por conexión, con tope donde el p99 supera 50 ms;
- pods para 5M = 5 000 000 / conexiones por pod, más margen.

Usa ese número para el `averageValue` del HPA en `k8s/base/hpa.yaml`.

### Ejecución B: el HPA escala por conexiones

```bash
task load:scaling     # HPA 2-6 réplicas, objetivo de 500 por pod, 3 000 conexiones en 3 escalones (~13 min)
```

Observa *Open connections per pod vs HPA target* y *HPA replicas*: las réplicas deberían subir con cada
escalón hasta 6. Los pods nuevos solo reciben conexiones nuevas, así que las líneas por pod convergen despacio.

### Resultados

Ejecución de humo (2026-11-01, 2 pods, 1 000 conexiones, `THINK_TIME=4`, 90 s): 1 000 conexiones mantenidas
(`http_connections_open` = VUs de k6), 26 233 peticiones, 0 fallidas, p99 de segmento 33 ms. El HPA leyó
501/500 conexiones por pod. Coste para k6: ~0.25 MiB y ~1.6 ms de CPU por petición, lo que fija
los valores por defecto del Job (`THINK_TIME=6`).

#### Ejecución A (2026-11-01, 1 pod, `THINK_TIME=6`, segmento de 64 KiB servido desde memoria)

Media de cada escalón en régimen estable (muestras cada 30 s desde Prometheus):

| Conexiones | Memoria del pod | CPU del pod | Peticiones/s | p99 servidor | p99 k6 (cliente) | CPU del nodo* |
| --- | --- | --- | --- | --- | --- | --- |
| 1 500 | 61 MiB | 0.15 cores | 250 | 1.1 ms | 46 ms | 1.2 |
| 3 000 | 116 MiB | 0.26 cores | 498 | 2.1 ms | 76 ms | 1.5 |
| 4 500 | 172 MiB | 0.34 cores | 691 | 20 ms | 3 s | 1.9 (Docker: 4.4 cores, 5.6 de 6 GiB) |

\* Suma de los contenedores según Prometheus; `docker stats` del nodo de minikube mide además el
sistema y el runtime.

0 errores y 0 peticiones descartadas (503) en toda la prueba. El escalón de 6 000 no se ejecutó: en el
de 4 500 **el nodo de minikube llegó a su límite** (4.4 cores de 4 y 5.6 de 6 GiB según Docker) y se
paró la prueba para no provocar que el kernel matase pods. El pod usaba solo 0.34 cores: la subida
del p99 a 4 500 es del nodo, no del servicio. Por eso el Job usa ahora 4 500 conexiones en 3 escalones.

**Coste por conexión** (pendiente entre escalones, casi lineal):

- **Memoria: ~38 KiB por conexión** (55.5 MiB por cada 1 500 conexiones, igual en los dos tramos).
- **CPU: ~0.53 ms por petición** (segmento de 64 KiB desde memoria), es decir, ~0.09 milicores por
  conexión con un segmento cada 6 s: **~11 000 conexiones por core**.

**Capacidad por pod con los recursos actuales** (límite de 768 MiB, 256 MiB de presupuesto para la
caché de cuerpos, ~30 MiB de runtime, 20 % de margen): (768 − 256 − 30) × 0.8 / 38 KiB ≈ **10 000
conexiones por memoria**. La CPU no tiene límite, pero el HPA escala al 60 % de la *request* de CPU
(250m), es decir, a **~1 700 conexiones**: con esos recursos la CPU disparaba el escalado
mucho antes que el objetivo provisional de 5 000 conexiones.

**Aplicado** (2026-11-01, `k8s/base`): request de CPU de 1 core y objetivo de **6 000 conexiones por
pod**. A 6 000 conexiones un pod usaría ~0.53 cores (53 % < 60 %, así que la CPU queda como red de
seguridad) y ~500 MiB de 768 MiB incluso con la caché de cuerpos llena.

**Extrapolación a 5M**: 5 000 000 / 6 000 ≈ **830 pods**, unos 1 000 con un 20 % de margen
(~1 000 cores, ~500 GiB de memoria reservada). El límite real a esa escala no es el servicio sino el
**ancho de banda**: 5M × 64 KiB cada 6 s ≈ 55 GB/s (~440 Gb/s). Sin una CDN delante que absorba la
mayor parte de los segmentos, el origen no es viable; con ella, el origen solo sirve los fallos de
caché de la CDN y necesita una fracción de esos pods.

**Límites de la medida**: sin TLS (en producción añade memoria por conexión y CPU por cifrado, salvo
que lo termine el balanceador), un único tamaño de segmento, todo servido desde la caché en memoria,
y extrapolación lineal desde 4 500 conexiones en un nodo compartido con k6 y la monitorización.

#### Ejecución B (2026-11-01, HPA 2-6 réplicas, objetivo 500 conexiones por pod, 3 000 conexiones en 3 escalones)

| Hora (UTC) | Conexiones | Pods | Conexiones por pod (media) | p99 servidor | p99 k6 |
| --- | --- | --- | --- | --- | --- |
| 13:10–13:12 | 1 000 | 2 | 501 | 0.8 ms | 29–40 ms |
| 13:13:28 | rampa a 2 000 | 2 → 3 | > 500 | 0.9 ms | 33 ms |
| 13:13:58 | | 3 → 4 | | | |
| 13:14–13:17 | 2 000 | 4 | 501 | 0.6–1.5 ms | 17–33 ms |
| 13:17:27 | rampa a 3 000 | 4 → 5 | > 500 | 1.1 ms | 59 ms |
| 13:17:57 | | 5 → 6 | | | |
| 13:18–13:21 | 3 000 | 6 | 501 | 1.6–1.9 ms | 36–45 ms |

239 679 peticiones, 100 % correctas, 0 respuestas 5xx, p99 de segmento 44 ms (umbral de 50 ms
cumplido). Todos los escalados los provocó la métrica de conexiones (evento del HPA: *pods metric
http_connections_open above target*), cada uno unos 30 s después de que la media pasara de 500.

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
entre todos los pods, también entre los antiguos. Los dos primeros pods acabaron con más del doble
del objetivo. Con el objetivo de producción (6 000) eso significaría pods a ~13 000 conexiones,
por encima de los ~10 000 que permite la memoria. Para corregirlo se ha implementado una **edad
máxima de conexión** (`MAX_CONNECTION_AGE_SECS`, [configuracion.md](servicio/configuracion.md#variables-de-entorno)):
pasado ese tiempo, la siguiente respuesta lleva `Connection: close` y el cliente se reconecta a
cualquier pod. Resultado en la ejecución siguiente.

Al bajar los VUs de k6, las conexiones no se cerraron hasta que terminó el proceso de k6: un VU
detenido no cierra su conexión keep-alive.

**Reducción:** la carga terminó a las 13:22. El HPA esperó su ventana de estabilización de 5 min y
bajó 6 → 4 (13:27:03), 4 → 3 (13:27:53) y 3 → 2 (13:29:00). El primer paso quita 2 pods porque el
20 % de 6 (1,2) se redondea hacia arriba.

#### Ejecución B con edad máxima de conexión (2026-11-01, 14:01–14:15 UTC, `MAX_CONNECTION_AGE_SECS=120`)

Mismas condiciones que la ejecución B anterior. Reparto entre pods (pod más cargado / menos cargado):

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
  quedaron hasta un 10 % por debajo de los espectadores (1 800 de 2 000). Con 300 s en producción
  las rachas son menores, pero el objetivo del HPA debe dejar ese margen.
- **Latencia: regresión sin causa identificada.** p99 del cliente de 1.1 s (44 ms en la ejecución
  sin edad máxima); mediana de 1 ms y p95 de 97 ms. La subida se concentra entre las 14:12:30 y las
  14:14, en 3 de los 6 pods (p99 del servidor de ~2 s), con esos pods a solo 0.05 cores. Descartado:
  presión de CPU o memoria del nodo (PSI de node-exporter sin espera de memoria), carga de LocalStack
  (respuestas en ~0.3 s), y la tasa de fallos de caché (igual en las dos ejecuciones y en todos los
  pods, no solo en los lentos). **Pendiente:** repetir A/B en las mismas condiciones con
  `MAX_CONNECTION_AGE_SECS=0` y `120` para atribuir la regresión (o descartarla como ruido del nodo)
  antes de llevar la edad máxima a producción.

### Conclusiones y extrapolación

**El servicio en Rust no es el límite para llegar a los objetivos del README.** Lo que decide si se
llega es el ancho de banda, el reparto de conexiones entre pods y la infraestructura alrededor del
servicio: CDN, balanceador y escalado de nodos.

#### Lo que han demostrado las pruebas

| Hallazgo | Dato | Qué significa |
| --- | --- | --- |
| Memoria por conexión | ~38 KiB, lineal | 5M de conexiones ≈ 181 GiB de memoria en la flota |
| CPU por petición | ~0.53 ms (segmento de 64 KiB desde memoria) | ~11 000 espectadores por core con un segmento cada 6 s |
| Latencia dentro del servicio | p99 de 1–2 ms con carga normal | El objetivo de p99 < 50 ms se cumple con mucho margen |
| Errores | 0 (ejecución A + 239 679 peticiones en la B) | Se cumple el objetivo de < 0.1 % |
| Escalado por conexiones | 2 → 6 pods, ~30 s después de pasar el objetivo; vuelta a 2 a los 5 min | El HPA por conexiones funciona |
| Reparto entre pods | Los pods más antiguos con 2.2 veces el objetivo; con edad máxima de 120 s, ±5 % en 1–2 min | Corregido, pero con una regresión de latencia (p99 1.1 s) aún sin explicar |
| Latencia del cliente en minikube | 30–45 ms normal; 3–5 s con el nodo saturado | Mide los límites del portátil, no los del servicio |

#### Extrapolación a los objetivos del README

**5M de conexiones simultáneas.**

- **Con el segmento de la prueba** (64 KiB cada 6 s): 5M / 6 000 conexiones por pod ≈ **830
  pods**, unos 1 000 con margen. Eso son ~1 000 cores de requests, ~830 000 peticiones/s y
  **~440 Gb/s** de salida.
- **Con vídeo real** el segmento de la prueba es irrealmente pequeño: 64 KiB cada 6 s son ~87 kb/s,
  y un vídeo HD a 3 Mb/s mueve unas **35 veces más datos**. 5M de espectadores a 3 Mb/s son
  **~15 Tb/s**. Los 6 000 espectadores de un pod necesitarían 18 Gb/s, más de lo que lleva la
  tarjeta de red de un nodo típico para todos sus pods. A esa escala la capacidad por pod la fija
  la red, no la CPU ni la memoria: unos pocos miles de espectadores por nodo.
- **Conclusión:** ningún clúster de origen sirve directamente a 5M de espectadores de vídeo. La
  arquitectura realista pone una **CDN delante**: la CDN mantiene los 5M de conexiones en sus puntos
  de presencia, y este servicio pasa a ser el **origen**, que solo sirve lo que la CDN no tiene en
  caché. Con un 95–99 % de aciertos de caché, el origen recibe entre el 1 % y el 5 % del tráfico:
  decenas de pods, y conexiones que vienen de la CDN, no de millones de usuarios.

**Latencia de 300 ms a 50 ms.**

- Dentro del servicio ya se cumple de sobra (1–2 ms).
- La latencia que ve un usuario depende sobre todo de la distancia de red hasta el servidor, y eso
  solo lo reduce una CDN con puntos de presencia cercanos.
- El punto de partida de 300 ms nunca se ha medido, así que la mejora aún no se puede demostrar.
  Haría falta una línea base (usuarios servidos desde una sola región, sin CDN) y compararla con la
  CDN en marcha.

**Ahorro de 100 000 $/mes.**

No se puede verificar, pero la escala está clara:

- **Cómputo:** ~1 000 cores serían del orden de decenas de miles de dólares al mes (estimación
  aproximada, sin precios detallados).
- **Datos salientes:** a 440 Gb/s son unos **143 PB al mes**, que cuestan mucho más que el cómputo.
- **El ahorro sale de la caché:** más aciertos de caché en la CDN significa menos tráfico al origen y
  menos pods. Optimizar el servicio en Rust ahorra poco al lado de eso.

#### Cambios necesarios, por prioridad

1. **CDN delante (CloudFront).** El servicio ya envía `Cache-Control`. Medir la tasa de aciertos de
   la CDN y dimensionar el origen con el tráfico que queda. Es el cambio que hace viables los 5M.
2. **Edad máxima de conexión:** implementada (`MAX_CONNECTION_AGE_SECS`, 300 s por defecto, 120 s en
   local) y verificada: reparte los pods a ±5 % del objetivo. Pendiente: explicar la regresión de
   latencia de la ejecución B con edad máxima antes de llevarla a producción.
3. **Balanceador:** un NLB (L4) delante del origen, y decidir dónde se cifra (TLS): en el NLB o en
   el pod. TLS no se ha medido y añade memoria y CPU por conexión.
4. **Escalado del propio clúster:** subir el límite del HPA (la base tiene un máximo de 20 pods),
   añadir escalado de nodos (Karpenter o Cluster Autoscaler), usar nodos con suficiente ancho de
   banda, y ajustar los límites del kernel por nodo (ficheros abiertos, tabla de conntrack).
5. **Escalar antes de los eventos conocidos.** El HPA reacciona ~30 s después de que llegue la carga,
   más lo que tarden en añadirse nodos. Para un estreno a las 20:00, subir el mínimo de pods unos
   minutos antes con un Schedule de Temporal ([temporal.md](operaciones/temporal.md#otros-workflows-candidatos)).
6. **Pruebas de carga realistas:** k6 en máquinas separadas (o el k6 operator repartido en varios
   nodos), segmentos de tamaño real (1–3 MB), TLS, y un catálogo mayor que los 256 MiB de caché en
   memoria para que parte de las peticiones vayan a S3. Repetir la ejecución A en esas condiciones
   para sustituir las cifras de 38 KiB y 0.53 ms.
7. **Monitorización de producción fuera del clúster** (AMP + Amazon Managed Grafana) y una forma de
   que el HPA lea la métrica de conexiones desde ahí
   ([monitorizacion.md](operaciones/monitorizacion.md#monitorización-en-producción)).

## Resultados (2026-10-19, portátil, Docker Desktop con una VM de 8 CPU, LocalStack 4.14)

Los resultados de Kubernetes (e2e a través del Ingress, reinicio progresivo bajo carga, HPA) están en
[kubernetes.md](operaciones/kubernetes.md#resultados-2026-10-19-minikube-4-cpu--6-gb-en-docker-desktop).

| Prueba | Resultado |
| --- | --- |
| e2e (suite de bash anterior, sustituida por `src/tests/e2e.rs`), servicio en el host | 46/46 |
| e2e, servicio en Docker (seguimiento de presign omitido) | 45/45, estable en 4 ejecuciones |
| Coalescing, 200 peticiones concurrentes en frío | 1 `HeadObject` + 1 `GetObject` |
| Load shedding (descarte de carga) (`MAX_CONCURRENT_REQUESTS=4`, 100 concurrentes) | 31 × 206, 69 × 503 (las 69 rechazadas en 2.5 ms en total); `/healthz` 200 en 11–33 ms mientras tanto |
| Apagado ordenado, 64 MiB a 1 MB/s, SIGTERM a mitad de descarga | Completa, el SHA-256 coincide; drenado 49 s + 5 s de vaciado, código de salida 0 |
| TLS contra el S3 real de AWS desde la imagen distroless | Handshake OK (S3 respondió 403 a las credenciales falsas) |
| Tamaño de la imagen | 64.4 MB (distroless `cc-debian13`, uid 65532 no root) |
| k6, objetos pequeños en caché a 2 000 req/s | p50 0.84 ms, p99 39 ms, 0 errores; CPU del servicio ≈ 0.8 cores |
| k6, objetos pequeños en caché a 4 000 req/s | p50 1.4 ms, p99 77 ms, 0 errores. k6 (3.1 cores) saturó la VM y perdió iteraciones. |
| k6, mixto con 20% de `Range` | rutas en caché p99 12–16 ms; `Range` p99 ≈ 3 s, limitado por LocalStack al 100% de CPU |
| k6, 200 VUs sin pausa (saturación) | 6 400 req/s, 243k peticiones, 0 errores, 16 MiB de memoria del servicio |

El objetivo de p99 < 50 ms se cumple mientras la máquina no está saturada. La cifra de 4 000 req/s dice más
del portátil (k6 y el servicio comparten una VM) que del servicio. Usa máquinas separadas
para obtener cifras reales.

## Hallazgos

1. **Corregido: las descargas se cortaban en el apagado.** axum drenaba todas las conexiones correctamente, pero el
   proceso salía en cuanto se *escribía* el último byte. La cola sin enviar del buffer de envío
   del kernel (~1.2 MiB aquí) se perdía cuando desaparecía la red del contenedor. Ahora el servicio espera
   `SHUTDOWN_FLUSH_SECS` (5 por defecto) después de drenar. En Kubernetes,
   `terminationGracePeriodSeconds` debe cubrir drenado + vaciado: la prueba de 64 MiB necesitó 54 s, y el
   periodo de gracia por defecto es de 30 s. El Deployment fija 60 s ([kubernetes.md](operaciones/kubernetes.md#el-deployment)).
2. **Abierto: bloqueos al caducar la caché.** Cuando termina el TTL de una entrada caliente, las siguientes peticiones esperan a S3
   (agrupadas, pero siguen esperando). Con TTL de 2 s y un LocalStack sobrecargado, el p99 de objetos pequeños
   llegó a 700 ms. Con 2+ pods la caché de cada pod caduca a su propio ritmo, así que los bloqueos
   se multiplican. Servir la entrada caducada mientras se refresca en segundo plano
   (stale-while-revalidate) eliminaría estos bloqueos. moka no lo hace de forma nativa, así que hace falta
   algo de código en `content.rs`.
3. **Por investigar:** unos 285 µs de CPU por petición en caché a 4 000 req/s es más de lo
   esperado. Perfilar (p. ej. `cargo flamegraph`) en hardware dedicado antes de optimizar.
