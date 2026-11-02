# Estado del proyecto

A fecha de 2026-11-01. Cada punto enlaza al detalle; las cifras completas están en
[pruebas.md](pruebas.md).

## Objetivos del README

| Objetivo | Estado |
| --- | --- |
| p99 < 50 ms | ✅ En local: 1–2 ms en el servicio, 44 ms en el cliente con 3 000 conexiones |
| Tasa de error < 0.1 % | ✅ 0 errores en todas las pruebas |
| 5M de conexiones simultáneas | 🟡 Extrapolado: ~1 000 pods de 6 000 conexiones, y una CDN delante (5M en HD son ~15 Tb/s) ([conclusiones](pruebas.md#conclusiones-y-extrapolación)) |
| Latencia de 300 ms a 50 ms | ⚪ Sin medir: falta la línea base de 300 ms |
| Ahorro de 100 000 $/mes | ⚪ No verificable sin producción; depende de la tasa de aciertos de la CDN |
| Temporal | 🟡 Primer workflow en local ([temporal.md](operaciones/temporal.md)) |
| Despliegue en AWS EKS | ⚪ Overlay preparado, sin desplegar |

## Hecho

**Servicio** ([servicio/](servicio))
- [x] Entrega desde S3 con `Range`, ETag/304, `Cache-Control` y URLs prefirmadas
- [x] Cachés en memoria con agrupación de peticiones (200 peticiones en frío → 1 llamada a S3)
- [x] Streaming con memoria acotada, load shedding, apagado ordenado sin cortar descargas
- [x] Métrica de conexiones abiertas y edad máxima de conexión (reparto ±5 % entre pods tras escalar)

**Despliegue y monitorización** ([operaciones/](operaciones))
- [x] minikube con Kustomize, LocalStack y despliegue sin reinicios si nada cambia (`task deploy:local`)
- [x] HPA por conexiones por pod (Prometheus Adapter), CPU como red de seguridad; validado 2 → 6 pods
- [x] Capacidad medida: ~38 KiB y ~0.09 milicores por conexión → 6 000 por pod con 1 core
- [x] Prometheus, Grafana (2 dashboards), alertas; diseño de monitorización para producción
- [x] Temporal: `PrescaleForEvent` con reintentos, temporizador duradero, compensación y lease anti-solape; sobrevive a borrar el worker

**Pruebas y herramientas**
- [x] 4 unitarias, 18 e2e en Rust (Compose, host o clúster), 6 del workflow de Temporal
- [x] Pruebas de carga y de conexiones simultáneas como Jobs de k6
- [x] `Taskfile.yml` como punto de entrada único, también desde PowerShell
- [x] Documentación en español por temas, con [registro de decisiones](servicio/decisiones.md) y [lecciones](lecciones.md)

## Pendiente

**Antes de nada**
- [ ] Primer commit (el repositorio no tiene ninguno) y CI con `task lint`, `test`, `e2e` y `temporal:test`
- [ ] Explicar la regresión de latencia con la edad máxima de conexión: prueba A/B ([detalle](pruebas.md#ejecución-b-con-edad-máxima-de-conexión-2026-11-01-14011415-utc-max_connection_age_secs120))

**Decisiones antes de EKS** ([abiertas](operaciones/decisiones.md#decisiones-abiertas))
- [ ] Entrada: CDN (CloudFront) → NLB (L4) → pods, y dónde se termina TLS
- [ ] Cómo llega la métrica de conexiones al HPA con Amazon Managed Prometheus (proxy SigV4 o KEDA)

**Despliegue en EKS**
- [ ] Completar el overlay `eks` (ECR, IRSA, bucket) y sustituir el Ingress por un NLB
- [ ] Escalado de nodos (Karpenter), `maxReplicas` acorde al dimensionado, límites del kernel por nodo
- [ ] Monitorización fuera del clúster y alerta "dead man's switch" ([detalle](operaciones/monitorizacion.md#monitorización-en-producción))

**Validación antes del lanzamiento**
- [ ] Pruebas de carga distribuidas con segmentos reales (1–3 MB), TLS y catálogo mayor que la caché
- [ ] Línea base de latencia sin y con CDN; tasa de aciertos de la CDN para dimensionar el origen

**Mejoras del servicio**
- [ ] Stale-while-revalidate para los parones al caducar la caché ([hallazgos](pruebas.md#hallazgos))
- [ ] Reciclar también HTTP/2 (GOAWAY) y cerrar conexiones inactivas ([limitaciones](servicio/api-http.md#limitaciones))
- [ ] Revisar `MAX_CONCURRENT_REQUESTS` con la capacidad medida; perfilar la CPU por petición

**Temporal**
- [ ] Servidor de producción (Temporal Cloud o Helm con PostgreSQL), mTLS, versionado de workflows
- [ ] Que el pre-escalado prepare también nodos ([qué cambia en producción](operaciones/temporal.md#qué-cambia-en-producción))
- [ ] Siguiente workflow: `PublishContent` ([candidatos](operaciones/temporal.md#otros-workflows-candidatos))
