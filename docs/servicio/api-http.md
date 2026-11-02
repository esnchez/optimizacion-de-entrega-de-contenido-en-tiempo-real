# API HTTP

## Contenido

### `GET /content/{key}`

Devuelve el objeto almacenado en `key` dentro del bucket configurado. `key` puede contener barras
(`/content/videos/2026/intro.mp4`).

| Cabecera de la petición | Comportamiento |
| --- | --- |
| `Range: bytes=start-end` | Se reenvía a S3. Responde `206 Partial Content` con `Content-Range`. La usan los reproductores de vídeo/audio para saltar a una posición. |
| `If-None-Match: "<etag>"` | `304 Not Modified` si el objeto no ha cambiado (sin cuerpo, sin llamada a S3) |

| Cabecera de la respuesta | Origen |
| --- | --- |
| `Content-Type`, `Content-Length`, `ETag`, `Last-Modified` | Metadatos del objeto en S3 |
| `Accept-Ranges: bytes` | Siempre |
| `Cache-Control` | Ajuste `CACHE_CONTROL` (por defecto `public, max-age=300`) |
| `Content-Range` | En las respuestas `206` |
| `x-request-id` | UUID generado, o el que envió el cliente |

| Estado | Significado |
| --- | --- |
| `200` | Objeto completo |
| `206` | Contenido parcial (petición con rango) |
| `304` | La copia en caché del cliente está al día |
| `400` | Clave no válida (vacía o de más de 1024 bytes) |
| `404` | El objeto no existe (este resultado se guarda en caché durante `METADATA_CACHE_TTL_SECS`) |
| `416` | Rango fuera del objeto; incluye `Content-Range: bytes */<size>` |
| `502` | Fallo de S3 (detalles solo en los logs) |
| `503` | Pod en su límite de concurrencia (load shedding); reintentar |
| `504` | Respuesta no lista dentro de `REQUEST_TIMEOUT_MS` |

### `HEAD /content/{key}`

Mismas cabeceras que `GET`, sin cuerpo. Se responde desde la caché de metadatos.

### `GET /url/{key}`

`307 Temporary Redirect` a una URL de S3 prefirmada válida durante `PRESIGN_TTL_SECS`. Los
clientes que siguen redirecciones (navegadores, `curl -L`, la mayoría de los reproductores)
descargan directamente de S3. `404` si el objeto no existe.

### Conexiones keep-alive

Las conexiones HTTP/1.1 se reutilizan entre peticiones. Cuando una conexión supera
`MAX_CONNECTION_AGE_SECS` (300 s por defecto, menos un 0–20 % aleatorio), la siguiente respuesta lleva
`Connection: close` y el servidor cierra la conexión después de enviarla. La respuesta es completa;
el cliente abre una conexión nueva para la siguiente petición, que el balanceador puede llevar a
otro pod. Los clientes HTTP habituales (navegadores, reproductores, k6) lo hacen solos.

### Limitaciones

- **Solo HTTP/1.1.** Las conexiones HTTP/2 no se reciclan: necesitarían un `GOAWAY`. Afecta a
  clientes que usen HTTP/2 directamente contra el origen; si el balanceador termina HTTP/2, no.
- **Las conexiones inactivas no se reciclan ni se cierran.** El cierre va en la siguiente respuesta,
  y no hay timeout de inactividad: una conexión abandonada sigue contando para el HPA y ocupando
  memoria.
- **`http_connections_open` se queda corto durante el reciclado.** Un espectador cuya conexión se
  cerró no cuenta hasta su siguiente segmento: hasta un 10 % menos con 120 s de edad máxima (menos
  con los 300 s de producción). El objetivo del HPA debe dejar ese margen.

## Operaciones

| Endpoint | Propósito | Notas |
| --- | --- | --- |
| `GET /healthz` | Liveness probe | Siempre `200` mientras el proceso está en marcha |
| `GET /readyz` | Readiness probe | `503` en cuanto empieza el apagado |
| `GET /metrics` | Scrape de Prometheus | Formato de exposición de texto |

Los endpoints operativos no están sujetos al load shedding ni al timeout de petición. En
Kubernetes **el Ingress no los enruta** (devuelven 404 desde fuera). El kubelet les hace las
probes y Prometheus los scrapea directamente en el pod; usa `kubectl port-forward` para llegar a
ellos.

Detrás de ingress-nginx, `x-request-id` es el ID de 32 caracteres hexadecimales generado por
nginx, que el servicio conserva. El mismo ID aparece entonces en ambos logs.

## Ejemplos

```bash
curl -i  http://localhost:8080/content/hello.txt
curl -I  http://localhost:8080/content/videos/big.bin
curl -i  -H 'Range: bytes=0-1023' http://localhost:8080/content/videos/big.bin
curl -i  -H 'If-None-Match: "5eb63bbbe01eeed093cb22bb8f5acdc3"' http://localhost:8080/content/hello.txt
curl -iL http://localhost:8080/url/videos/big.bin -o big.bin
```
