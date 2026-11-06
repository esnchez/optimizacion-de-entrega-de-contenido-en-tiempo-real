# Arquitectura

## Dónde se sitúa el servicio

```
 Usuarios ──► Ingress (solo /content, /url) ──► Service ──► N × pods Rust (HPA) ──► AWS S3
                                                  │
                                                  └──► /metrics ──► Prometheus ──► Grafana
```

Cada pod es **sin estado** (stateless): todo lo que guarda en memoria (cachés) se puede reconstruir desde S3.
Cualquier pod puede atender cualquier petición, así que Kubernetes puede añadir y quitar pods libremente (HPA).

## Dentro de un pod

```
 Socket TCP (TCP_NODELAY, contado mientras está abierto)  ── connections.rs
   │
   ▼
 hyper (framing HTTP/1.1 + HTTP/2)                       ── axum::serve
   │
   ▼
 SetRequestId → Trace → PropagateRequestId               ── tower-http, se aplica a todas las rutas
   │
   ▼
 track_metrics (recuento, latencia, en curso)            ── telemetry.rs
   │
   ├── /healthz /readyz /metrics                         ── nunca se aplica load shedding
   │
   └── CORS → Timeout → LoadShed + ConcurrencyLimit
          │
          ▼
       handlers (routes.rs)
          │
          ▼
       ContentService (content.rs)
          ├── caché de metadatos (moka)  ── fallo ──► S3 HeadObject
          ├── caché de cuerpos pequeños (moka) ── fallo ──► S3 GetObject (cargado entero en memoria)
          └── stream ───────────────────────────────► S3 GetObject (enviado en streaming, trozo a trozo)
```

## Flujo de una petición: `GET /content/{key}`

1. **Validar la clave** (no vacía, ≤ 1024 bytes, el límite de S3).
2. **Buscar los metadatos** en la caché de metadatos. Si no están, se hace *una sola* llamada `HeadObject` aunque
   lleguen miles de peticiones para la misma clave en el mismo instante (request coalescing, agrupación de peticiones).
   El "no encontrado" también se guarda en caché, así que una avalancha de peticiones para una clave inexistente no llega a S3.
3. **Petición condicional**: si `If-None-Match` coincide con el ETag → `304 Not Modified`, sin cuerpo
   y sin llamada a S3.
4. **Objeto pequeño, sin `Range`** (≤ `SMALL_OBJECT_MAX_BYTES`, por defecto 1 MiB): se sirve desde la
   caché de cuerpos. Si no está, el objeto completo se descarga una sola vez, también agrupando las peticiones.
5. **Objeto grande o petición con `Range`**: `GetObject` reenviando el `Range` del cliente, y el
   cuerpo de la respuesta de S3 se envía en **streaming** al cliente en trozos de `STREAM_CHUNK_BYTES` (por defecto 64 KiB).
   Nada se carga entero en memoria, así que la memoria por petición se queda en aproximadamente un trozo.
6. `416` de S3 → devolvemos `416` con `Content-Range: bytes */<size>`.

## Ruta alternativa: `GET /url/{key}`

Devuelve `307 Temporary Redirect` a una **URL prefirmada de S3** (validez por defecto de 5 min). Después el cliente
descarga directamente desde S3, así que los bytes nunca pasan por nuestros pods. Úsala para ficheros muy grandes
en los que el ancho de banda del proxy sería el cuello de botella. La contrapartida: no hay caché ni métricas sobre los bytes,
y el cliente ve una URL de S3.

## Responsabilidades de los módulos

| Módulo | Responsabilidad | Habla con |
| --- | --- | --- |
| `main.rs` | Montaje y ciclo de vida: construirlo todo, enlazar el puerto, servir, apagar de forma limpia | todo |
| `config.rs` | Leer las variables de entorno una sola vez y convertirlas en un `Config` tipado | — |
| `routes.rs` | Solo aspectos HTTP: enrutado, cabeceras, códigos de estado, orden de los middleware | `ContentService` |
| `content.rs` | Política de caché y agrupación de peticiones | `S3Store` |
| `storage.rs` | Llamadas al SDK de S3 y conversión de los errores del SDK a `AppError` | AWS S3 |
| `state.rs` | Estado compartido que se pasa a los handlers | — |
| `telemetry.rs` | Logs y métricas | — |
| `connections.rs` | Envoltorio del listener que cuenta las conexiones TCP abiertas (`http_connections_open`) y recicla las que superan `MAX_CONNECTION_AGE_SECS` (`Connection: close`) | — |
| `error.rs` | Un enum de errores → respuestas HTTP | — |

La separación en capas (`routes` → `content` → `storage`) permite sustituir S3 más adelante (p. ej. por un nivel
de caché en disco local) tocando solo `storage.rs`.

## Carpetas del repositorio

```
content-delivery/         # el servicio en Rust (crate content-delivery, binario app)
├── Cargo.toml            # dependencias, perfil release, lints
├── Cargo.lock            # versiones exactas de las dependencias (en el repo: es un binario)
├── rust-toolchain.toml   # fija la toolchain stable + rustfmt/clippy
├── rustfmt.toml          # reglas de formato
├── .cargo/config.toml    # RUST_LOG por defecto para `cargo run`
├── Dockerfile            # build multi-stage -> imagen distroless (operaciones/imagen-de-contenedor.md)
├── src/                  # los módulos de la tabla de arriba
└── tests/e2e.rs          # pruebas end-to-end contra LocalStack (operaciones/pruebas.md)
temporal/                 # worker de Temporal en Go: workflow PrescaleForEvent (operaciones/temporal.md)
├── workflow.go           # el workflow: lease, subir mínimo del HPA, esperar pods, mantener, restaurar
├── activities.go         # las activities: llamadas a la API de Kubernetes (lease en anotaciones del HPA)
├── kube.go               # cliente mínimo de la API de Kubernetes (token del ServiceAccount)
├── main.go               # el worker: conexión, registro, métricas en :9090
├── workflow_test.go      # pruebas con el entorno de pruebas de Temporal
└── Dockerfile
k8s/                      # Kustomize: base, overlays local/eks, monitoring, temporal (operaciones/kubernetes.md)
loadtest/                 # k6.js y los Jobs de k6 en k8s/ (operaciones/pruebas.md)
docker-compose.yml        # LocalStack + imagen del servicio (operaciones/entorno-local.md)
Taskfile.yml              # punto de entrada: pruebas, entorno, despliegue, carga
```

## Apagado ordenado

Al recibir `SIGTERM` (lo que envía Kubernetes) el servicio:

1. cambia `/readyz` a `503` para que el Service deje de enviar tráfico nuevo al pod,
2. deja de aceptar conexiones nuevas,
3. espera a que las peticiones en curso (incluidos los streams activos) se escriban por completo,
4. espera `SHUTDOWN_FLUSH_SECS` (por defecto 5 s) para que los datos que siguen en los buffers de socket del kernel lleguen a los
   clientes lentos, y después termina. Sin este paso, las pruebas mostraron que se perdía el último ~1 MiB de una descarga grande
   cuando se desmontaba la red del contenedor.

En Kubernetes, un `sleep` de 5 s en `preStop` se ejecuta antes del SIGTERM para que la retirada del endpoint llegue primero al
ingress, y `terminationGracePeriodSeconds: 60` acota toda la secuencia
([kubernetes.md](../operaciones/kubernetes.md#el-deployment)). Verificado: un reinicio progresivo (rolling restart) bajo carga produjo
0 peticiones fallidas de 230k.
