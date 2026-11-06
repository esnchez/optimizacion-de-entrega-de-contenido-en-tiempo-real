# Documentación

Documentos cortos, uno por tema, agrupados en dos carpetas. Los comandos para empezar están en el
[README del proyecto](../README.md) y en `task` (lista todas las tareas del
[`Taskfile.yml`](../Taskfile.yml)).

```
docs/
├── estado.md       # qué está hecho y qué falta
├── glosario.md     # términos y documentos donde aparece cada uno
├── servicio/       # el servicio en Rust: arquitectura, API, configuración, concurrencia, decisiones
└── operaciones/    # ejecutarlo, desplegarlo, observarlo y probarlo, con los resultados de las pruebas
```

## Resultados y extrapolación a los objetivos del README

Resumen de las pruebas del 2026-10-19 al 2026-11-05; el detalle de cada ejecución está en
[pruebas](operaciones/pruebas.md) y el estado frente a cada objetivo en [estado](estado.md). Las
cifras de capacidad son las del 2026-11-05, con k6 fuera del clúster; las del 2026-11-01 (k6 dentro
del nodo) se mantienen en pruebas como referencia.

### Veredicto

**Se cumple todo lo que depende del servicio y se puede probar en local**: p99 < 50 ms (3 ms con
6 000 conexiones por pod, 17 ms con 9 000), 0 errores en todas las pruebas, escalado por conexiones
con reparto equilibrado entre pods, y un punto de saturación medido (9 000 conexiones por pod). **No
se pueden demostrar las cifras de *Impacto* del README**: los 5M de conexiones son una extrapolación
(~1 000 pods) que además exige una CDN delante por ancho de banda, y la mejora de 300 a 50 ms y el
ahorro de 100 000 $/mes necesitan líneas base y un entorno de producción que no existen. El servicio
en Rust no es lo que separa el proyecto de esos objetivos; lo son la infraestructura alrededor (CDN,
NLB, EKS con escalado de nodos) y las mediciones en producción. Estado de cada fase y de cada
objetivo: [estado](estado.md#objetivos-del-readme).

### Lo que demuestran las pruebas

| Pregunta | Respuesta | Evidencia |
| --- | --- | --- |
| ¿Cumple p99 < 50 ms? | **Sí, en el servidor**: 3 ms con 6 000 conexiones en un pod y 17 ms con 9 000. **En el cliente**, 22–32 ms con 3 000 conexiones repartidas en 6 pods; por encima de ~4 500 conexiones por pod el p99 del cliente lo marca el portátil saturado (96 ms a 6 000) | [ejecución A con k6 fuera](operaciones/pruebas.md#ejecución-a-con-k6-fuera-del-clúster-2026-11-05), [A/B](operaciones/pruebas.md#ab-de-la-edad-máxima-con-k6-fuera-del-clúster-2026-11-05) |
| ¿Tasa de error < 0,1 %? | **Sí**: 0 errores en todas las pruebas (1,25 M peticiones solo en la ejecución A con 9 000 conexiones), también durante reinicios progresivos y escalados | [pruebas](operaciones/pruebas.md#ejecuciones) |
| ¿Cuántas conexiones aguanta un pod? | **9 000 medidas sin errores** (282 de 768 MiB, 1,3 núcleos, p99 del servidor 17 ms). El tope de 4 500 del 2026-11-01 era el nodo, con k6 dentro | [ejecución A con k6 fuera](operaciones/pruebas.md#ejecución-a-con-k6-fuera-del-clúster-2026-11-05) |
| ¿Cuánto cuesta una conexión simultánea? | **~33 KiB**, lineal hasta 9 000. **CPU: ~0,6 ms por segmento hasta 4 500 conexiones y 1–1,3 ms por encima**, con el pod esperando CPU (PSI del 8 al 23 %) | ídem |
| ¿Escala por conexiones? | **Sí**: el HPA pasa de 2 a 6 pods ~30 s después de superar el objetivo y vuelve a 2 a los 5 min; igual con k6 dentro y fuera | [ejecución B](operaciones/pruebas.md#ejecución-b-el-hpa-escala-por-conexiones-2026-11-01), [A/B](operaciones/pruebas.md#ab-de-la-edad-máxima-con-k6-fuera-del-clúster-2026-11-05) |
| ¿Se reparten las conexiones entre pods? | **Solo con edad máxima de conexión, y sin coste**: sin ella los pods antiguos llegan a 2,2 veces el objetivo (1 092 / 25 con objetivo 500); con 120 s quedan a ±5 % en cada escalón. La regresión de p99 1,1 s del 2026-11-01 **no se reproduce** con k6 fuera: era el nodo | [A/B](operaciones/pruebas.md#ab-de-la-edad-máxima-con-k6-fuera-del-clúster-2026-11-05) |
| ¿Dónde está el cuello de botella en local? | **En el portátil, no en el servicio**: con k6 dentro, el nodo de 4 núcleos (k6 e ingress-nginx gastaban más CPU que el pod); con k6 fuera, los 8 hilos del portátil a partir de ~6 000 conexiones (nodo y k6 a 6 núcleos de media, 10 en picos) | [pruebas](operaciones/pruebas.md#ejecución-a-con-k6-fuera-del-clúster-2026-11-05) |

Las latencias de cliente dependen del portátil. Para extrapolar se usa el **coste por conexión
medido dentro del pod** y, para la CPU, la cifra alta (con contención), que es la conservadora.

### Coste medido por pod

| Recurso | Medido (2026-11-05) | Con los recursos de `k8s/base` (request de 1 core, límite de 768 MiB) |
| --- | --- | --- |
| Memoria | ~33 KiB por conexión; ~12 MiB en reposo; 187 MiB a 6 000 y 282 MiB a 9 000 conexiones | 768 MiB − 256 MiB de caché de cuerpos − ~30 MiB de runtime, con 20 % de margen: **~12 000 conexiones** |
| CPU | 0,46 núcleos a 4 500, **0,71 a 6 000** y 1,29 a 9 000 conexiones (≈ 0,12 milicores por conexión a 6 000) | HPA al 60 % de 1 core = 0,6 núcleos: **la CPU dispara el escalado hacia ~5 300 conexiones**, antes que el objetivo de 6 000 |
| Latencia en el servidor | p99 3 ms a 6 000, 17 ms a 9 000 | dentro de los 50 ms hasta al menos 9 000 |
| **Objetivo del HPA** | | **6 000 conexiones por pod** se mantiene, con la request de CPU subida a **1,25 cores** (60 % = 0,75 > 0,71) para que la CPU siga siendo solo red de seguridad |

El 2026-11-01, con k6 dentro del nodo, la CPU medía ~0,09 milicores por conexión (0,53 núcleos a
6 000) y fijó la request de 1 core. Con más conexiones y el portátil más cargado la cifra sube; hasta
medirla en hardware dedicado se dimensiona con la alta.

### Extrapolación a los objetivos del README

"5M" son **conexiones simultáneas** (espectadores conectados a la vez), no peticiones por segundo.

| Objetivo | Con el segmento de la prueba (64 KiB cada 6 s) | Con vídeo HD real (3 Mb/s) |
| --- | --- | --- |
| Pods de origen para 5M | 5 000 000 / 6 000 ≈ **830 pods**, ~1 000 con 20 % de margen | lo fija la red, no la CPU ni la memoria: unos pocos miles de espectadores por nodo |
| Recursos | ~1 250 cores de request (1,25 por pod) y ~450 GiB de memoria (≈ 190 MiB de conexiones + 256 MiB de caché por pod); ~157 GiB de memoria de conexiones en la flota | — |
| Peticiones | ~830 000 req/s en la flota | igual |
| Ancho de banda de salida | **~440 Gb/s** (~143 PB al mes) | **~15 Tb/s**: 6 000 espectadores de un pod serían 18 Gb/s, más que la tarjeta de red de un nodo |

**Conclusión: el servicio en Rust no es el límite.** Un pod sirve 9 000 conexiones con p99 de 17 ms
y 0 errores en un portátil compartido, pero ningún clúster de origen sirve directamente a 5M de
espectadores de vídeo. Lo que decide si se llega es el ancho de banda, el reparto de conexiones
entre pods (ya resuelto con la edad máxima) y la infraestructura alrededor del servicio:

- **CDN delante** (CloudFront): mantiene los 5M de conexiones en sus puntos de presencia, y este
  servicio pasa a ser el **origen**. Con un 95–99 % de aciertos de caché el origen recibe del 1 al
  5 % del tráfico: decenas de pods, con conexiones que vienen de la CDN. El servicio ya envía
  `Cache-Control` para eso.
- **Latencia de 300 ms a 50 ms**: dentro del servicio ya se cumple de sobra (p99 de 1–17 ms según la
  carga). La latencia que ve un usuario depende sobre todo de la distancia de red, y solo la reduce
  una CDN con puntos de presencia cercanos. La línea base de 300 ms no se ha medido.
- **Ahorro de 100 000 $/mes**: no verificable sin producción. Los datos salientes (~143 PB al mes con
  el segmento de la prueba) cuestan mucho más que el cómputo (~1 250 cores, del orden de decenas de
  miles de dólares al mes). El ahorro sale de la tasa de aciertos de la CDN, no de optimizar el servicio.

### Cambios necesarios, por prioridad

| # | Cambio | Por qué |
| --- | --- | --- |
| 1 | **CDN delante** y medir su tasa de aciertos para dimensionar el origen | Es el cambio que hace viables los 5M |
| 2 | **Request de CPU de 1,25 cores** en `k8s/base` (o volver a medir la CPU en hardware dedicado) | Con lo medido el 2026-11-05 la CPU dispararía el HPA hacia ~5 300 conexiones, antes que el objetivo de 6 000 |
| 3 | **Edad máxima de conexión** en producción (`MAX_CONNECTION_AGE_SECS`, 300 s) | Validada: reparte a ±5 % sin errores ni regresión; sin ella los pods antiguos superarían la memoria (~13 000 conexiones con objetivo 6 000) |
| 4 | **NLB (L4)** delante de los pods y decidir dónde se termina TLS | Detrás de un proxy L7 los pods solo ven su pool keep-alive; TLS no se ha medido |
| 5 | **Escalado del clúster**: `maxReplicas` acorde (la base tiene 20), Karpenter o Cluster Autoscaler, nodos con ancho de banda suficiente, límites del kernel (ficheros abiertos, conntrack) | 830+ pods no caben en 20 réplicas |
| 6 | **Pre-escalado antes de eventos conocidos** con Temporal (`PrescaleForEvent`, hecho en local) | El HPA reacciona ~30 s tarde, más lo que tarden los nodos |
| 7 | **Pruebas de carga realistas**: k6 en otra máquina, segmentos de 1–3 MB, TLS, catálogo mayor que la caché | Sustituir las cifras de CPU, que en el portátil dependen de la contención |
| 8 | **Monitorización fuera del clúster** (AMP + Amazon Managed Grafana) y cómo llega la métrica de conexiones al HPA | [monitorización en producción](operaciones/monitorizacion.md#monitorización-en-producción) |

### Qué no se ha medido

- El servicio en **hardware dedicado** con k6 en otra máquina: k6 ya corre fuera del clúster, pero en
  el mismo portátil, que se satura por encima de ~6 000 conexiones. Es la prueba que fijaría la CPU
  por conexión y el p99 del cliente.
- **TLS**, segmentos de tamaño real y un catálogo mayor que la caché en memoria.
- Más de 9 000 conexiones en un pod, y la edad máxima de 300 s de producción (se probó con 120 s).
- Las líneas base de "300 ms" y del coste actual del README: no hay sistema anterior con el que comparar.

## Empieza aquí

| Documento | Responde a |
| --- | --- |
| [estado.md](estado.md) | ¿Qué está hecho y qué falta? |
| [Arquitectura](servicio/arquitectura.md) | ¿Dónde se sitúa el servicio, cómo fluye una petición, qué módulos y carpetas hay? |
| [glosario.md](glosario.md) | Términos usados en la documentación y dónde aparece cada uno |

## [servicio/](servicio): el código

Para entender o cambiar [`content-delivery/`](../content-delivery). Orden de lectura sugerido:

| Documento | Responde a |
| --- | --- |
| [Arquitectura](servicio/arquitectura.md) | Flujo de una petición del socket a S3, módulos, carpetas del repositorio, apagado ordenado |
| [API HTTP](servicio/api-http.md) | Endpoints, cabeceras, códigos de estado, conexiones keep-alive y sus limitaciones |
| [Configuración](servicio/configuracion.md) | Variables de entorno y permisos de S3 |
| [Concurrencia y escalado](servicio/concurrencia-y-escalado.md) | ¿Cómo atiende a muchos usuarios? E/S asíncrona, cachés, agrupación de peticiones, streaming, backpressure |
| [Estructuras de datos](servicio/estructuras-de-datos.md) | Los tipos del núcleo (`AppState`, cachés, `Bytes`, streams, errores) |
| [Dependencias](servicio/dependencias.md) | Cada crate, por qué se eligió, y las métricas que exporta el servicio |
| [Decisiones del servicio](servicio/decisiones.md) | ¿Por qué está hecho así? |

## [operaciones/](operaciones): ejecutar, desplegar, observar y probar

| Documento | Responde a |
| --- | --- |
| [Entorno local](operaciones/entorno-local.md) | Docker Compose, el servicio en el host, S3 falso, comandos de desarrollo |
| [Imagen de contenedor](operaciones/imagen-de-contenedor.md) | Cómo se construye la imagen y por qué (multi-stage, distroless, sin root), etiquetado |
| [Kubernetes](operaciones/kubernetes.md) | Manifiestos de Kustomize, minikube, cada decisión de Kubernetes, problemas encontrados |
| [Monitorización](operaciones/monitorizacion.md) | Prometheus, Grafana, Prometheus Adapter, el dashboard, alertas, monitorización en producción |
| [Manual de operaciones](operaciones/manual-de-operaciones.md) | Runbook: desplegar, revertir, señales de salud, errores, depuración |
| [Temporal](operaciones/temporal.md) | Workflow `PrescaleForEvent`: conceptos, despliegue, cómo saber que funciona, experimentos |
| [Pruebas y resultados](operaciones/pruebas.md) | Unitarias, e2e, carga y conexiones con k6: cómo se ejecutan y qué dieron |
| [Decisiones de operación](operaciones/decisiones.md) | ¿Por qué se despliega, se observa y se prueba así? |

Las decisiones están numeradas en orden cronológico para todo el proyecto y repartidas entre las
dos carpetas según su tema.
