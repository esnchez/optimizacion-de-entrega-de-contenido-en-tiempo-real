# Estado del proyecto

Qué está hecho y qué falta. El detalle del código está en [servicio/](servicio), el despliegue,
la monitorización y las pruebas con sus resultados en [operaciones/](operaciones)
([pruebas](operaciones/pruebas.md)).

## Objetivos del README

A fecha de 2026-11-05, con las pruebas del 2026-10-19 al 2026-11-05. Pods necesarios por objetivo:
[extrapolación](README.md#extrapolación-a-los-objetivos-del-readme).

| Objetivo | Estado |
| --- | --- |
| Fase 1: arquitectura y métricas de rendimiento | ✅ [arquitectura](servicio/arquitectura.md), métricas propias y [dashboard](operaciones/monitorizacion.md#cómo-leer-el-dashboard) |
| Fase 2: sistema en Rust con eventos en Temporal | ✅ servicio en Rust · 🟡 Temporal con un primer workflow en local ([temporal](operaciones/temporal.md)) |
| Fase 3: Kubernetes y optimización de latencia | ✅ desplegado y medido en minikube · ⚪ EKS sin desplegar |
| Fase 4: monitoreo y ajuste continuo | ✅ Prometheus, Grafana y alertas en local; capacidad por pod medida y aplicada · ⚪ monitorización de producción diseñada, sin hacer |
| p99 < 50 ms | ✅ en el servidor: 3 ms con 6 000 conexiones por pod y 17 ms con 9 000 · ✅ en el cliente: 22–32 ms con 3 000 conexiones en 6 pods · ⚪ con más de ~4 500 conexiones por pod, el p99 del cliente lo marca el portátil saturado ([resultados](README.md#lo-que-demuestran-las-pruebas)) |
| Tasa de error < 0,1 % | ✅ 0 errores en todas las pruebas, también en reinicios y escalados |
| 5M de conexiones simultáneas | 🟡 extrapolado, no medido: ~1 000 pods de 6 000 conexiones, y una CDN delante (5M en HD son ~15 Tb/s) ([extrapolación](README.md#extrapolación-a-los-objetivos-del-readme)) |
| Punto de saturación por pod | ✅ 9 000 conexiones medidas sin errores (p99 del servidor 17 ms); ~12 000 por memoria; la CPU (60 % de 1 core) llegaría hacia ~5 300, así que la request debe subir a 1,25 cores para el objetivo de 6 000 ([coste](README.md#coste-medido-por-pod)) |
| Latencia de 300 ms a 50 ms | ⚪ sin medir: falta la línea base de 300 ms |
| Ahorro de 100 000 $/mes | ⚪ no verificable sin producción; depende de la tasa de aciertos de la CDN |
| Despliegue en AWS EKS | ⚪ overlay preparado, sin desplegar |
| Pruebas unitarias y de integración | ✅ 4 unitarias, 18 e2e (Compose, host y clúster), 6 del workflow de Temporal |

## Checklist — no podemos olvidar

**Servicio** ([servicio/](servicio))
- [x] Entrega desde S3 con `Range`, ETag/304, `Cache-Control` y URLs prefirmadas
- [x] Cachés en memoria con agrupación de peticiones (200 peticiones en frío → 1 llamada a S3)
- [x] Streaming con memoria acotada, load shedding, apagado ordenado sin cortar descargas
- [x] Métrica de conexiones abiertas y edad máxima de conexión (reparto ±5 % entre pods tras escalar, sin regresión de latencia en el A/B)
- [ ] Stale-while-revalidate para los parones al caducar la caché ([hallazgos](operaciones/pruebas.md#hallazgos))

**Kubernetes y monitorización** ([operaciones/](operaciones))
- [x] minikube con Kustomize, LocalStack y despliegue sin reinicios si nada cambia (`task deploy:local`)
- [x] HPA por conexiones por pod (Prometheus Adapter), CPU como red de seguridad; validado 2 → 6 pods
- [x] Prometheus, Grafana (2 dashboards) y alertas
- [x] Probes que no dependen de S3, parada ordenada (`preStop` + 60 s de gracia), Pod Security `restricted`
- [ ] Alerta "dead man's switch" y monitorización fuera del clúster ([producción](operaciones/monitorizacion.md#monitorización-en-producción))

**Temporal**
- [x] `PrescaleForEvent` con reintentos, temporizador duradero, compensación y lease anti-solape; sobrevive a borrar el worker
- [ ] Que el pre-escalado prepare también nodos ([qué cambia en producción](operaciones/temporal.md#qué-cambia-en-producción))

**Pruebas y herramientas**
- [x] Pruebas de carga y de conexiones simultáneas como Jobs de k6 y con k6 fuera del clúster, con métricas en Prometheus
- [x] `Taskfile.yml` como punto de entrada único, también desde PowerShell
- [ ] CI con `task lint`, `test`, `e2e` y `temporal:test`

## Pendiente

| Tema | Estado | Nota |
| --- | --- | --- |
| Request de CPU de 1,25 cores en `k8s/base` (o medir la CPU en hardware dedicado) | ⏳ | con 1 core la CPU dispararía el HPA antes del objetivo de 6 000 ([coste](README.md#coste-medido-por-pod)) |
| Entrada en producción: CDN → NLB (L4) → pods, y dónde se termina TLS | ⏳ | antes de EKS ([decisiones abiertas](operaciones/decisiones.md#decisiones-abiertas)) |
| Métrica de conexiones para el HPA con Amazon Managed Prometheus | ⏳ | proxy SigV4 o KEDA |
| Overlay `eks` completo (ECR, IRSA, bucket) e Ingress sustituido por un NLB | ⏳ | `task deploy:eks` se niega mientras queden `<...>` |
| Escalado de nodos (Karpenter), `maxReplicas` acorde, límites del kernel por nodo | ⏳ | [cambios necesarios](README.md#cambios-necesarios-por-prioridad) |
| Pruebas de carga distribuidas con segmentos reales, TLS y catálogo mayor que la caché | ⏳ | sustituye las cifras medidas en el portátil |
| Línea base de latencia sin y con CDN; tasa de aciertos de la CDN | ⏳ | para los objetivos de 300 → 50 ms y de ahorro |
| Reciclar también HTTP/2 (GOAWAY) y cerrar conexiones inactivas | ⏳ | [limitaciones](servicio/api-http.md#limitaciones) |
| Revisar `MAX_CONCURRENT_REQUESTS` con la capacidad medida; perfilar la CPU por petición | ⏳ | [hallazgos](operaciones/pruebas.md#hallazgos) |
| Allocator alternativo (mimalloc) | ⏸ | solo si el profiling lo justifica |
| Servidor de Temporal de producción (Temporal Cloud o Helm con PostgreSQL), mTLS, versionado | ⏳ | [producción](operaciones/temporal.md#qué-cambia-en-producción) |
| Siguiente workflow: `PublishContent` | ⏳ | [candidatos](operaciones/temporal.md#otros-workflows-candidatos) |
