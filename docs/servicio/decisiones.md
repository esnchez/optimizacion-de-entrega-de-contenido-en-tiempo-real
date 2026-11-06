# Decisiones del servicio

Cada decisión importante, en el orden en que se tomó, con el motivo y dónde están los detalles.
Cuando una decisión cambie, añade una entrada nueva que sustituya a la anterior en lugar de editar
el historial. La numeración es común a todo el proyecto: las decisiones de despliegue, monitorización y
Temporal están en [operaciones/decisiones.md](../operaciones/decisiones.md).

| # | Fecha | Decisión | Por qué (resumen) | Detalles |
| --- | --- | --- | --- | --- |
| 1 | 2026-10-17 | El crate de Rust vive en `src/` (binario llamado `app`) | El Dockerfile existente construye desde `src/` y ejecuta `app` | [arquitectura.md](arquitectura.md#carpetas-del-repositorio) (sustituida por la #48) |
| 2 | 2026-10-17 | **axum + tokio + tower** para el servidor HTTP | E/S asíncrona para muchas conexiones con pocos hilos; middleware estándar de tower | [dependencias.md](dependencias.md#pila-del-servidor-http) |
| 3 | 2026-10-17 | **`aws-sdk-s3` oficial** con endpoint configurable opcional | Cadena de proveedores incl. IRSA; el mismo código contra LocalStack/MinIO | [dependencias.md](dependencias.md#aws) |
| 4 | 2026-10-17 | Capas `routes` → `content` → `storage` | HTTP, política de caché y acceso a S3 cambian por motivos distintos | [arquitectura.md](arquitectura.md#responsabilidades-de-los-módulos) |
| 5 | 2026-10-17 | Cachés **moka** para metadatos (incl. 404) y cuerpos pequeños (≤ 1 MiB, limitada en bytes) | El contenido caliente se sirve desde RAM; la caché negativa frena las avalanchas de 404 | [estructuras-de-datos.md](estructuras-de-datos.md#las-dos-cachés-contentrs) |
| 6 | 2026-10-17 | **Request coalescing** con `or_try_insert_with` | Una ráfaga sobre una clave fría → 1 llamada a S3. Verificado: 200 → 1. | [concurrencia-y-escalado.md](concurrencia-y-escalado.md#2-no-hacer-el-mismo-trabajo-dos-veces-caché--agrupación-de-peticiones) |
| 7 | 2026-10-17 | La clave de la caché de cuerpos incluye el **ETag** | Un objeto modificado nunca puede servirse con un cuerpo obsoleto | [estructuras-de-datos.md](estructuras-de-datos.md#las-dos-cachés-contentrs) |
| 8 | 2026-10-17 | Los objetos grandes y los rangos se sirven en **streaming**, nunca en búfer | Memoria por petición ≈ un fragmento de 64 KiB, sea cual sea el tamaño del objeto | [estructuras-de-datos.md](estructuras-de-datos.md#streaming-bytestream--asyncread--readerstream--body) |
| 9 | 2026-10-17 | **Redirección prefirmada** `/url/{key}` como camino alternativo | Los ficheros muy grandes no pasan por los pods | [api-http.md](api-http.md#get-urlkey) |
| 10 | 2026-10-17 | **Load shedding** (límite de concurrencia + 503 inmediato); rutas operativas exentas | Mantener rápidas las peticiones aceptadas en sobrecarga; no fallar la liveness bajo carga | [concurrencia-y-escalado.md](concurrencia-y-escalado.md#4-backpressure-y-load-shedding) |
| 11 | 2026-10-17 | Configuración desde **variables de entorno**, leída al arrancar | 12-factor, se corresponde con ConfigMaps; las erratas fallan al arrancar | [configuracion.md](configuracion.md) |
| 12 | 2026-10-17 | **Métricas Prometheus** con buckets de latencia en torno a 50 ms; plantilla de ruta como label | Medir el objetivo de p99; baja cardinalidad de labels | [dependencias.md](dependencias.md#observabilidad) |
| 13 | 2026-10-17 | Perfil release: fat LTO, 1 codegen unit, `panic = "abort"` | Binario más rápido y pequeño; un pod que hace panic se reinicia de todos modos | [Cargo.toml](../../content-delivery/Cargo.toml) |
| 15 | 2026-10-19 | Pruebas end-to-end como **script de bash + curl contra LocalStack** | Ejercita la API real de S3; el mismo script se ejecuta contra el host, Docker y Kubernetes | [pruebas.md](../operaciones/pruebas.md) |
| 16 | 2026-10-19 | Espera **`SHUTDOWN_FLUSH_SECS`** (5 s) tras el drenado, antes de salir | Las pruebas mostraron que se perdía el final de las descargas grandes cuando el proceso salía justo después de escribir | [pruebas.md](../operaciones/pruebas.md#hallazgos) |
| 32 | 2026-10-19 | **Buckets de latencia por debajo del milisegundo** en el servicio | El p50 es de ~55 µs; un bucket mínimo de 1 ms lo ocultaba | [monitorizacion.md](../operaciones/monitorizacion.md#decisiones) |
| 37 | 2026-11-01 | Métrica **`http_connections_open`**: el listener cuenta cada conexión TCP de `accept` a cierre (`connections.rs`) | `http_requests_in_flight` acaba en las cabeceras y no ve al espectador que espera entre segmentos | [arquitectura.md](arquitectura.md) |
| 41 | 2026-11-01 | Eliminados el `docker-compose.yml` y el `loadtest/k6.js` originales; documentación en español | No funcionaban (minikube en un contenedor; 5M VUs contra `/`); el README del proyecto está en español | [estado.md](../estado.md) |
| 43 | 2026-11-01 | **Edad máxima de conexión** (`MAX_CONNECTION_AGE_SECS`, 300 s; 120 s en local): la siguiente respuesta HTTP/1.1 lleva `Connection: close`, con un 0–20 % de variación por conexión | En la ejecución B los pods más antiguos acabaron con 2.2 veces el objetivo. Cerrar tras una respuesta completa no corta ninguna petición; la variación evita que miles de conexiones se reconecten en el mismo segundo | [pruebas.md](../operaciones/pruebas.md#ejecución-b-con-edad-máxima-de-conexión-2026-11-01) |
| 44 | 2026-11-01 | **Pruebas e2e en Rust** (hoy `content-delivery/tests/e2e.rs`) con **Docker Compose** como entorno y **Task** como punto de entrada; se elimina `src/scripts/e2e.sh` | El script de bash mezclaba cada comprobación con fontanería (curl, grep, logs de LocalStack) y era difícil de leer. En Rust cada prueba tiene nombre, sube sus propios objetos y cuenta llamadas a S3 con las métricas del servicio. Hurl se descartó: no cubre las ráfagas concurrentes ni las diferencias de métricas | [pruebas.md](../operaciones/pruebas.md#suite-end-to-end) |
| 48 | 2026-11-05 | **Estructura del repositorio como en `sistema-analisis-tiempo-real`**: el crate en `content-delivery/`, el worker en `temporal/`, los manifiestos en `k8s/`, Compose en `docker-compose.yml` y k6 en `loadtest/` (`k6.js` y `k8s/`), todo en la raíz. Sustituye a la #1 | `src/src/` confundía, y `k8s/` y `temporal/` no son parte del crate; cada componente desplegable tiene su carpeta y el contexto de build de la imagen solo contiene el crate | [arquitectura.md](arquitectura.md#carpetas-del-repositorio) |

## Decisiones abiertas

| Tema | Opciones | ¿Bloqueante? |
| --- | --- | --- |
| Parones al expirar la caché | Stale-while-revalidate en `content.rs` | No; mejora el p99 |
