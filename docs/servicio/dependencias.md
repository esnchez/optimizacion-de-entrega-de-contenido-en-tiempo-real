# Crates de Rust

Todas las versiones están fijadas en [`src/Cargo.toml`](../../src/Cargo.toml) y bloqueadas en `Cargo.lock`.

## Runtime asíncrono

### `tokio` (1.53)
El runtime asíncrono: un planificador multihilo con work-stealing más E/S no bloqueante construida sobre
epoll (Linux) / kqueue / IOCP.

- **Por qué:** un hilo del sistema operativo por conexión cuesta unos 8 MB de pila y un cambio de contexto cada vez.
  Una *tarea* de tokio cuesta unos cientos de bytes, así que un pod puede mantener decenas de miles de
  conexiones abiertas con un puñado de hilos (por defecto, un hilo de trabajo por núcleo de CPU).
- **Features usadas:** `rt-multi-thread` (planificador con work-stealing), `macros` (`#[tokio::main]`,
  `select!`), `net` (`TcpListener`), `signal` (Ctrl+C / SIGTERM), `time`.

### `tokio-util` (0.7, feature `io`)
Nos da `ReaderStream`, que convierte un `AsyncRead` (el cuerpo de S3) en un `Stream<Item = Bytes>`
(lo que necesita un cuerpo HTTP). Es el puente que hace posible el **streaming sin buffers intermedios**.

### `bytes` (1.12)
`Bytes` es un buffer de bytes inmutable con conteo de referencias. Clonarlo es O(1) (se incrementa un contador, sin
copia). El mismo objeto cacheado se puede enviar a 10 000 usuarios sin copiarlo 10 000 veces. Ver
[estructuras-de-datos.md](estructuras-de-datos.md#bytes).

## Pila del servidor HTTP

### `axum` (0.8)
Framework web del equipo de tokio, construido sobre `hyper` + `tower`.

- **Por qué axum en lugar de actix-web:** funciona sobre tokio sin más (sin runtime adicional), cada middleware es una
  capa estándar de `tower` (reutilizable, componible), los extractores se comprueban en tipos (`State`, `Path`,
  `HeaderMap`), y la sobrecarga por encima de hyper es muy pequeña.
- **Features usadas:** `http1`, `http2` (una conexión TCP puede llevar muchas peticiones concurrentes),
  `tokio`, `macros`.
- `axum::serve` controla hyper; `ListenerExt::tap_io` activa `TCP_NODELAY` en cada socket aceptado.

### `hyper` (1.x, a través de axum)
La implementación real de HTTP/1 y HTTP/2. No la usamos directamente, pero es la que interpreta las
peticiones y escribe las respuestas. Es una de las implementaciones HTTP más rápidas en cualquier lenguaje.

### `tower` (0.5)
La abstracción `Service` / `Layer` sobre la que se construye todo el middleware. Usamos sus capas de resiliencia:

| Capa | Propósito |
| --- | --- |
| `ConcurrencyLimitLayer` | Como máximo `MAX_CONCURRENT_REQUESTS` peticiones de contenido en curso por pod |
| `LoadShedLayer` | Cuando se alcanza el límite, rechaza *inmediatamente* en lugar de encolar |
| `HandleErrorLayer` (axum) | Convierte el error de load shedding en `503 Service Unavailable` |

Rechazar rápido bajo sobrecarga mantiene baja la latencia de las peticiones aceptadas, y el 503 indica al
balanceador de carga o al cliente que reintente en otro sitio. Ver [concurrencia-y-escalado.md](concurrencia-y-escalado.md).

### `tower-http` (0.7)
Middleware HTTP ya hecho:

| Capa | Propósito |
| --- | --- |
| `TraceLayer` | Un span de tracing por petición (método, URI, estado, latencia) |
| `SetRequestIdLayer` / `PropagateRequestIdLayer` | UUID `x-request-id` generado o conservado, devuelto en la respuesta, para correlacionar logs |
| `TimeoutLayer` | `504` si las cabeceras de la respuesta no están listas en `REQUEST_TIMEOUT_MS` |
| `CorsLayer` | Permite que reproductores en navegador de otros orígenes obtengan contenido (`GET`/`HEAD`) |

## AWS

### `aws-config` (1.x)
Resuelve las credenciales y la región con la cadena de proveedores estándar de AWS: variables de entorno → ficheros de perfil →
**web identity / IRSA** (cómo obtienen los pods en EKS roles de IAM sin claves estáticas) → ECS / EC2 IMDS.

### `aws-sdk-s3` (1.x, feature `rt-tokio`)
El cliente oficial de S3. Usamos `HeadObject`, `GetObject` (con `Range`) y el prefirmado de peticiones.

- Usa internamente un pool de conexiones (hyper + rustls), así que las conexiones a S3 se reutilizan entre
  peticiones: sin handshake TLS por petición.
- `ByteStream` (el cuerpo de la respuesta) es perezoso: los datos se piden a S3 solo a la velocidad a la que los enviamos
  al cliente, así que un cliente lento nunca nos obliga a cargar el objeto entero en memoria.
- `endpoint_url` + `force_path_style` permiten que el mismo código hable con MinIO / LocalStack / moto en local.

## Caché

### `moka` (0.12, feature `future`)
Una caché concurrente en memoria inspirada en Caffeine de Java.

- **Por qué moka:** lecturas sin bloqueos, alta tasa de aciertos (admisión TinyLFU + expulsión LRU), soporte de TTL,
  expulsión por tamaño mediante un *weigher* (acotamos la caché de cuerpos por **bytes**, no por entradas),
  y una API asíncrona.
- **Request coalescing (agrupación de peticiones):** `entry(key).or_try_insert_with(loader)` garantiza que cuando N tareas
  fallan en la misma clave a la vez, solo **una** ejecuta el loader (la llamada a S3) y las demás esperan
  su resultado. Es la principal protección contra los picos de "estampida" (thundering herd) sobre S3. En una prueba,
  200 peticiones concurrentes para una clave no cacheada hicieron exactamente 1 llamada `HeadObject` y 1 `GetObject`.

## Observabilidad

### `tracing` + `tracing-subscriber` (features `env-filter`, `json`)
Logging estructurado y compatible con async. Los spans siguen a una petición a través de los puntos `.await`. `RUST_LOG`
controla el nivel de detalle; `LOG_FORMAT=json` emite líneas JSON para la agregación de logs (Loki, CloudWatch…).

### `metrics` + `metrics-exporter-prometheus`
`metrics` es una fachada ligera (`counter!`, `histogram!`, `gauge!`); el exportador de Prometheus
guarda los valores y los expone en `GET /metrics`. Los buckets del histograma van de 100 µs a 5 s:
finos por debajo de 1 ms (las respuestas cacheadas tardan ~55 µs en p50) y densos alrededor del objetivo de **p99 < 50 ms**.

Métricas exportadas:

| Métrica | Tipo | Etiquetas |
| --- | --- | --- |
| `http_requests_total` | counter | method, path (plantilla de la ruta), status |
| `http_request_duration_seconds` | histogram | method, path, status |
| `http_requests_in_flight` | gauge (llegada de la petición → cabeceras de la respuesta) | — |
| `http_connections_open` | gauge (accept TCP → cierre); el HPA escala con ella | — |
| `http_connections_total` | counter (conexiones aceptadas) | — |
| `http_connections_recycled_total` | counter (conexiones cerradas por `MAX_CONNECTION_AGE_SECS`) | — |
| `http_requests_shed_total` | counter | — |
| `cache_requests_total` | counter | cache (`metadata`/`body`), result (`hit`/`miss`) |

## Errores

### `thiserror` (2.x)
Deriva `std::error::Error` y `Display` para `AppError` y `ConfigError` sin código repetitivo.
`AppError` también implementa `IntoResponse` de axum, así que los handlers pueden usar simplemente `?`.

## Crates que *no* se usan a propósito (de momento)

| Crate | Por qué no ahora |
| --- | --- |
| `actix-web` | Tiene su propio modelo de actores/runtime; axum encaja mejor con tower y tokio |
| `reqwest` | El SDK de AWS trae su propio cliente HTTP |
| `serde`/`config` para la configuración | Una docena de variables de entorno no necesitan un framework; `config.rs` tiene ~80 líneas |
| `mimalloc` / `jemallocator` | Un asignador más rápido puede ayudar con carga alta; se añadirá cuando las pruebas de carga muestren presión sobre el asignador |
| `temporalio-sdk` | Los workflows de Temporal (fase 2 del README) se ejecutan fuera del camino de las peticiones y tendrán su propio worker |
