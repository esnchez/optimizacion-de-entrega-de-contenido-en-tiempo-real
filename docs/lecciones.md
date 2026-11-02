# Lecciones aprendidas

Lo no evidente que se descubrió al construir y probar el proyecto. Una o dos líneas por lección; el
detalle está en el enlace.

## Escalado y capacidad

- **"5M" son conexiones, y a esa escala el límite es el ancho de banda, no el código.** 5M de
  espectadores en HD son ~15 Tb/s: sin una CDN delante no hay origen viable. ([pruebas](pruebas.md#conclusiones-y-extrapolación))
- **Detrás de un proxy L7 los pods no ven a los usuarios**, solo el pool keep-alive del proxy. Para
  escalar por conexiones, las conexiones deben llegar a los pods por L4 (NLB). ([kubernetes](operaciones/kubernetes.md#autoescalado-y-disponibilidad))
- **Las peticiones en curso no ven al espectador que espera entre segmentos.** Hay que contar
  conexiones en el listener, de `accept` a cierre. ([arquitectura](servicio/arquitectura.md))
- **La request de CPU decide qué métrica del HPA manda.** Con 250m, la CPU disparaba el escalado a
  ~1 700 conexiones, mucho antes que el objetivo de conexiones. ([pruebas](pruebas.md#ejecución-a-2026-11-01-1-pod-think_time6-segmento-de-64-kib-servido-desde-memoria))
- **Escalar no mueve las conexiones existentes.** Los pods antiguos acabaron con 2.2 veces el
  objetivo; cerrar las conexiones al cabo de un tiempo los reparte en 1–2 min. ([pruebas](pruebas.md#ejecución-b-con-edad-máxima-de-conexión-2026-11-01-14011415-utc-max_connection_age_secs120))
- **Medir antes de dimensionar.** El objetivo provisional (5 000) y la cifra "hipotética" de 50 000
  conexiones por pod quedaron sustituidos por 6 000, medido. ([pruebas](pruebas.md#conexiones-simultáneas))

## Medir en un portátil

- **En minikube, la latencia de cola mide el nodo.** k6 e ingress-nginx usaban más CPU que el
  servicio; a 4 500 conexiones el nodo, no el pod, era el límite. ([pruebas](pruebas.md#conexiones-simultáneas))
- **k6 reserva todos los VUs al arrancar** (~1.7 GiB con 4 500) y un VU detenido no cierra su
  conexión hasta que termina k6.
- **Las pruebas que cuentan conexiones necesitan margen en un clúster:** las sondas de Kubernetes y
  el scraping de Prometheus abren y cierran las suyas. ([pruebas](pruebas.md#suite-end-to-end))
- **Abrir 200 conexiones a través del port mapping de Docker Desktop puede tardar más de 2 s**, más
  que el TTL de la caché de las pruebas. ([pruebas](pruebas.md#suite-end-to-end))

## Kubernetes, Docker y Windows

- **Grafana 13 usa ~400 MiB en reposo.** Con un límite de 512 MiB, el recolector de Go entraba en
  bucle (2,8 cores) y Grafana dejaba de responder. ([monitorización](operaciones/monitorizacion.md#problemas-habituales))
- **Docker Desktop adjunta metadatos de procedencia con la hora del build**: el ID de la imagen
  cambia en cada build salvo con `--provenance=false`. ([imagen](operaciones/imagen-de-contenedor.md#etiquetado))
- **El nodo de minikube usa containerd**: las imágenes se consultan con `crictl`, no con `docker`.
- **Reiniciar minikube desactiva las métricas de ingress-nginx**; `task monitoring:install` las
  vuelve a activar. ([monitorización](operaciones/monitorizacion.md#instalación))
- **`curl.exe` de Windows no puede escribir en `/dev/null`** (código 23); el intérprete de Task sí
  traduce la redirección `> /dev/null`.
- **En PowerShell, `~/bin` no estaba en el PATH**: `kubectl` resolvía al 1.32 de Docker Desktop.
  ([manual](operaciones/manual-de-operaciones.md#entorno-local))
- **Go exige LF** (`gofmt`) aunque el resto del repositorio use CRLF.

## Temporal

- **El workflow solo decide; las activities hacen el trabajo** y deben ser idempotentes. Reintentos y
  temporizadores los guarda el servidor: el workflow sobrevivió a borrar su worker. ([temporal](operaciones/temporal.md#conceptos-en-este-código))
- **Un ID de workflow fijo no evita ejecuciones solapadas**: los Schedules añaden la hora al ID. El
  lock fiable es el propio recurso (un lease en sus anotaciones). ([temporal](operaciones/temporal.md#ejecuciones-solapadas-el-lease))
- **Una ejecución omitida no es un fallo**: termina bien y se cuenta aparte, para no disparar alertas.
- **La etiqueta `namespace` del SDK choca con la del pod** y Prometheus la renombra a
  `exported_namespace`: las consultas filtran por `job`. ([temporal](operaciones/temporal.md#2-métricas-y-alertas-prometheusgrafana-va-bien-en-general))
- **`localhost` resuelve también a `::1`** y el servidor de desarrollo solo escucha en IPv4: la CLI
  usa `127.0.0.1`.
- **El worker puede arrancar antes que el servidor**: reintentar la conexión evita reinicios del pod y
  falsas alertas.

## Proceso

- **Un listado truncado ocultó una carpeta.** `find | head` se llenó con `target/` y se concluyó que
  `src/temporal/` no existía. Excluir los artefactos de build antes de afirmar que algo no está.
- **Probar en todos los modos encuentra lo que uno solo no ve**: las pruebas e2e pasaban en el host y
  fallaban en Compose y en el clúster por motivos distintos.
