# Concurrencia y escalado

Cómo gestiona el servicio muchos usuarios simultáneos, primero dentro de un pod y después entre pods.

## 1. E/S asíncrona: muchas conexiones, pocos hilos

Servir contenido consiste casi por completo en **esperar**: al socket del cliente o a S3. Con tokio,
una petición en espera es una tarea suspendida (unos cientos de bytes), no un hilo del sistema operativo bloqueado. Un pod con
4 núcleos ejecuta 4 hilos de trabajo que van alternando entre decenas de miles de tareas. El planificador con
work-stealing mantiene ocupados todos los núcleos aunque algunas conexiones sean mucho más costosas que otras.

## 2. No hacer el mismo trabajo dos veces: caché + agrupación de peticiones

El tráfico real está sesgado: un conjunto pequeño de objetos "calientes" (el nuevo episodio, la miniatura popular)
recibe la mayoría de las peticiones.

| Mecanismo | Efecto |
| --- | --- |
| Caché de metadatos | `HEAD`, `304` y las comprobaciones de existencia nunca llegan a S3 cuando hay acierto |
| Caché de cuerpos pequeños | Los objetos pequeños calientes (≤ 1 MiB por defecto) se sirven desde RAM |
| Caché negativa | Las avalanchas de peticiones de claves inexistentes no llegan a S3 |
| Request coalescing | N fallos simultáneos sobre una clave → **1** petición a S3 |
| Compartición de `Bytes` | Una sola copia en memoria de un objeto cacheado, da igual cuántos lectores haya |

La agrupación de peticiones es lo que protege a S3 (y nuestra latencia) al inicio de un pico de tráfico, cuando las cachés
están frías y todo el mundo pide lo mismo a la vez. En una prueba local, 200 peticiones concurrentes
para una clave no cacheada produjeron exactamente una llamada `HeadObject` y una `GetObject`.

## 3. Memoria acotada: streaming

Los objetos grandes nunca se cargan enteros. El cuerpo es un stream basado en demanda (pull) (ver
[estructuras-de-datos.md](estructuras-de-datos.md#streaming-bytestream--asyncread--readerstream--body)),
así que la memoria por petición es de aproximadamente `STREAM_CHUNK_BYTES` (64 KiB). Cota superior aproximada para un pod:

```
memoria ≈ caché de cuerpos (256 MiB) + caché de metadatos (~100k entradas, decenas de MiB)
        + streams en curso × trozo (10 000 × 64 KiB ≈ 640 MiB en el peor caso)
```

Ajusta `MAX_CONCURRENT_REQUESTS`, `STREAM_CHUNK_BYTES` y `BODY_CACHE_MAX_BYTES` junto con el
límite de memoria del pod.

## 4. Backpressure y load shedding

```
petición ──► LoadShed ──► ConcurrencyLimit(MAX_CONCURRENT_REQUESTS) ──► handler
                │
                └── límite alcanzado → 503 inmediato (http_requests_shed_total++)
```

Sin un límite, un servidor sobrecargado lo acepta todo, las colas crecen y la latencia de **todos** los usuarios
sube hasta que los timeouts se encadenan. Con load shedding, las peticiones hasta el límite mantienen su
latencia normal y el resto falla rápido con `503`, que el ingress o el cliente pueden reintentar en otro
pod. Las rutas `/healthz`, `/readyz` y `/metrics` están **fuera** del limitador para que un pod sobrecargado
no muera por su liveness probe.

> Nota: el permiso de concurrencia se mantiene hasta que se generan las *cabeceras de la respuesta*. El stream del cuerpo
> de una descarga grande continúa después. Por tanto, las descargas largas están acotadas por la memoria (punto 3)
> y por los límites de recursos de Kubernetes, no por este contador.

`TimeoutLayer` (por defecto 10 s) devuelve `504` si las cabeceras de la respuesta no se generan a tiempo, así que una llamada lenta
a S3 no puede retener un permiso indefinidamente.

## 5. Ajustes a nivel de conexión

- **HTTP/2** está activado: los navegadores y el ingress pueden multiplexar muchas peticiones sobre una conexión.
- **TCP_NODELAY** en cada socket aceptado: las respuestas pequeñas y los primeros bytes de un stream salen
  inmediatamente en lugar de esperar al algoritmo de Nagle.
- **Pool de conexiones a S3** (en el SDK de AWS): las conexiones a S3 se mantienen abiertas y se reutilizan, sin
  handshake TLS por petición.
- **Perfil release**: `lto = "fat"`, `codegen-units = 1`, `opt-level = 3`, `panic = "abort"`, `strip = "symbols"`
  producen un binario más pequeño y más rápido.

## 6. Escalado horizontal (Kubernetes)

Los manifiestos concretos (HPA, PDB, estrategia de despliegue, probes) y el razonamiento detrás de ellos están en
[kubernetes.md](../operaciones/kubernetes.md). Los principios:

Cada pod es sin estado e independiente, así que la capacidad crece linealmente con las réplicas:

- **HPA** sobre las conexiones abiertas por pod (`http_connections_open`, vía Prometheus Adapter), con
  la CPU como red de seguridad, añade pods antes de que se alcance el límite por pod.
- **Readiness** (`/readyz`) saca los pods de la rotación durante el apagado, y los despliegues se vacían
  de forma limpia.
- **Cachés por pod** implica que cada pod nuevo arranca en frío, y una ráfaga en frío llega a S3 hasta una vez por
  pod (verificado con 2 pods: 2 HEAD + 2 GET para 200 peticiones concurrentes). La agrupación de peticiones mantiene barato el calentamiento. Si la
  tasa combinada de peticiones a S3 se convierte en un problema de coste, el siguiente paso es un nivel compartido (una CDN delante, o enrutado
  por hash consistente de la clave en el ingress para que una clave llegue siempre a los mismos pods).
- **Redirecciones prefirmadas** (`/url/{key}`) descargan por completo en S3 el ancho de banda de los ficheros grandes.

### Sobre el objetivo de 5M de conexiones concurrentes

Ningún proceso por sí solo mantiene 5M de conexiones; es una cifra de flota. Lo que cuesta una
conexión en un pod, cuántos pods harían falta y por qué a esa escala manda el ancho de banda (y una
CDN delante) está en la [extrapolación](../README.md#extrapolación-a-los-objetivos-del-readme). La
cabecera `Cache-Control` (`CACHE_CONTROL`, por defecto `public, max-age=300`) ya está pensada para
esa CDN.
