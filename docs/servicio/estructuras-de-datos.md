# Estructuras de datos

Estos son los tipos en torno a los que está construido el servicio y por qué se eligió cada uno.

## `AppState` — estado compartido (`state.rs`)

```rust
#[derive(Clone)]
pub struct AppState {
    pub config:  Arc<Config>,
    pub content: Arc<ContentService>,
    pub metrics: PrometheusHandle,   // internamente basado en Arc
    ready:       Arc<AtomicBool>,
}
```

- axum clona el estado en cada petición. Como todos los campos son un `Arc`, un clon son solo unos
  pocos incrementos atómicos del contador de referencias, sin copia profunda.
- **Ningún `Mutex` en todo el camino de la petición.** La configuración es de solo lectura tras el arranque, las cachés son
  concurrentes internamente y el estado de readiness es un `AtomicBool`. Los locks son lo que suele impedir que un
  servidor concurrente escale con más núcleos; no tenemos ninguno por el que competir.

### `Arc<T>`
Puntero *Atomically Reference Counted* (con conteo atómico de referencias): propiedad compartida entre hilos. Los datos viven mientras
algún handler los siga teniendo, así que no hay lifetimes que gestionar a través de los puntos `.await`.

### `AtomicBool` (indicador de readiness)
Se escribe una vez (en el apagado) y lo lee cada probe de `/readyz`. `Ordering::Relaxed` es suficiente porque
no protege ninguna otra memoria; es solo un indicador.

## `Config` (`config.rs`)

Un struct simple construido una sola vez por `Config::from_env()`. Los valores se convierten a los tipos adecuados
(`SocketAddr`, `Duration`, `u64`) al arrancar, así que una errata en una variable de entorno falla rápido en el arranque en lugar
de en la primera petición.

## `ObjectMeta` (`storage.rs`)

```rust
pub struct ObjectMeta {
    pub size: u64,
    pub content_type: Option<String>,
    pub etag: Option<String>,
    pub last_modified: Option<String>, // fecha HTTP ya formateada
}
```

Lo mínimo necesario para responder a `HEAD` e `If-None-Match`, y para decidir entre memoria y streaming.
`last_modified` se formatea una sola vez al guardarlo en caché, no en cada respuesta.

## Las dos cachés (`content.rs`)

```rust
metadata: moka::future::Cache<String, Option<Arc<ObjectMeta>>>,
bodies:   moka::future::Cache<String, Bytes>,
```

| | Caché de metadatos | Caché de cuerpos |
| --- | --- | --- |
| Clave | clave del objeto | `"{key}\0{etag}"` |
| Valor | `Option<Arc<ObjectMeta>>` | `Bytes` |
| Acotada por | número de entradas (`METADATA_CACHE_CAPACITY`) | **bytes totales** (`BODY_CACHE_MAX_BYTES`) mediante un weigher |
| Caducidad | TTL (`METADATA_CACHE_TTL_SECS`) | TTL (`BODY_CACHE_TTL_SECS`) |
| Expulsión | admisión TinyLFU + LRU | admisión TinyLFU + LRU |

Detalles de diseño:

- **`Option` en el valor** guarda en caché los resultados negativos: una clave inexistente se guarda como `None`, así que
  los 404 repetidos no cuestan nada.
- **`Arc<ObjectMeta>`**: los aciertos de caché entregan un puntero, no una copia de las cadenas que contiene.
- **ETag en la clave de la caché de cuerpos**: cuando un objeto cambia en S3, su nuevo ETag produce una clave nueva
  y el cuerpo antiguo nunca se puede servir para la versión nueva. La entrada antigua simplemente caduca.
- **TinyLFU** solo admite una entrada nueva si es probable que se use más que la que expulsaría.
  Por tanto, un recorrido puntual de objetos poco pedidos no vacía de la caché el conjunto caliente.
- **La agrupación de peticiones (coalescing)** ocurre dentro de `or_try_insert_with`: la primera tarea ejecuta el loader, y las tareas
  concurrentes para la misma clave esperan a que termine. Por eso `AppError` es `Clone`: un mismo fallo se comparte con
  todas las tareas en espera.

## `Bytes`

`bytes::Bytes` es un trozo de memoria inmutable con conteo de referencias.

- `clone()` es O(1): incrementa un contador y no copia datos. Un objeto cacheado de 1 MiB servido a 10 000
  usuarios concurrentes ocupa **1 MiB de memoria**, no 10 GiB.
- hyper escribe `Bytes` directamente en el socket.

## Streaming: `ByteStream` → `AsyncRead` → `ReaderStream` → `Body`

```rust
let reader = object.body.into_async_read();                  // ByteStream de aws -> AsyncBufRead
let stream = ReaderStream::with_capacity(reader, 64 * 1024); // -> Stream<Item = io::Result<Bytes>>
Response::new(Body::from_stream(stream))                     // -> cuerpo HTTP
```

Es un **pipeline basado en demanda (pull) con backpressure (contrapresión) natural**: hyper pide el siguiente trozo solo
cuando el socket del cliente puede aceptarlo, y solo entonces leemos más de S3. Un cliente con una conexión
móvil lenta nunca nos obliga a guardar en memoria más de ~un trozo. La memoria por petición en streaming está acotada por
`STREAM_CHUNK_BYTES` (más los buffers del socket), sea cual sea el tamaño del objeto.

## `AppError` (`error.rs`)

```rust
#[derive(Debug, Clone, thiserror::Error)]
pub enum AppError { NotFound, RangeNotSatisfiable, InvalidKey, Storage(String) }
```

Un conjunto cerrado de fallos, cada uno asociado exactamente a un código de estado (`404`, `416`, `400`, `502`).
Los detalles de almacenamiento se registran en los logs pero nunca se envían a los clientes, así que no se filtran los nombres internos de los buckets ni el texto
de los errores de AWS.

## `Router` + `ServiceBuilder` (capas de tower)

La pila de middleware es en sí misma una estructura de datos: tipos `Service` anidados que el compilador
ensambla en tiempo de compilación. No hay dispatch dinámico por capa ni una lista de middleware en tiempo de ejecución que
recorrer, así que cada petición simplemente atraviesa una cadena de llamadas a funciones inlined.
