# Documentación

Agrupada por tema. Los comandos para empezar están en el [README del proyecto](../README.md) y en
`task` (lista todas las tareas del [`Taskfile.yml`](../Taskfile.yml)).

```
docs/
├── estado.md       # qué está hecho y qué falta, en una lista
├── pruebas.md      # todas las pruebas, sus resultados y la extrapolación a 5M
├── lecciones.md    # lo no evidente que se aprendió por el camino
├── servicio/       # el servicio en Rust: cómo funciona por dentro
└── operaciones/    # desplegarlo, observarlo y operarlo: Kubernetes, monitorización, Temporal
```

## Empieza aquí

| Documento | Qué cubre |
| --- | --- |
| [estado.md](estado.md) | Objetivos del README frente a la realidad, y la lista de lo hecho y lo pendiente |
| [pruebas.md](pruebas.md) | Cómo se ejecutan las pruebas (unitarias, e2e, carga, conexiones simultáneas, apagado), sus resultados y las conclusiones |
| [lecciones.md](lecciones.md) | Las lecciones de la sesión, una o dos líneas cada una, con enlace al detalle |

## [servicio/](servicio): el servicio en Rust

Para entender o cambiar el código de [`src/src/`](../src/src). Orden de lectura sugerido:

| Documento | Qué cubre |
| --- | --- |
| [arquitectura.md](servicio/arquitectura.md) | Dónde se sitúa el servicio, flujo de una petición del socket a S3 y de vuelta, responsabilidades de cada módulo, apagado ordenado |
| [api-http.md](servicio/api-http.md) | Endpoints, cabeceras, códigos de estado, conexiones keep-alive y sus limitaciones |
| [configuracion.md](servicio/configuracion.md) | Variables de entorno y cómo ejecutarlo en local contra un S3 falso |
| [concurrencia-y-escalado.md](servicio/concurrencia-y-escalado.md) | Cómo atiende a muchos usuarios: E/S asíncrona, cachés y agrupación de peticiones, streaming, backpressure |
| [estructuras-de-datos.md](servicio/estructuras-de-datos.md) | Los tipos del núcleo (`AppState`, cachés, `Bytes`, streams, errores) |
| [dependencias.md](servicio/dependencias.md) | Cada crate de Rust, por qué se eligió, y las métricas que exporta el servicio |
| [decisiones.md](servicio/decisiones.md) | Decisiones del servicio y decisiones abiertas |

## [operaciones/](operaciones): desplegar, observar y operar

Para llevar el servicio a un clúster y saber cómo se comporta. Orden de lectura sugerido:

| Documento | Qué cubre |
| --- | --- |
| [imagen-de-contenedor.md](operaciones/imagen-de-contenedor.md) | Cómo se construye la imagen Docker y por qué (multi-stage, distroless, sin root), etiquetado |
| [kubernetes.md](operaciones/kubernetes.md) | Manifiestos de Kustomize, clúster local con minikube, cada decisión de Kubernetes, validación |
| [monitorizacion.md](operaciones/monitorizacion.md) | Prometheus, Grafana y Prometheus Adapter, el dashboard, alertas, y la monitorización en producción |
| [manual-de-operaciones.md](operaciones/manual-de-operaciones.md) | Runbook: desplegar, revertir, señales de salud, errores, depuración, entorno local |
| [temporal.md](operaciones/temporal.md) | Workflow `PrescaleForEvent`: conceptos, despliegue, cómo saber que funciona, experimentos, otros candidatos |
| [decisiones.md](operaciones/decisiones.md) | Decisiones de Kubernetes, imagen, monitorización, pruebas de carga y Temporal, y decisiones abiertas |

Las decisiones están numeradas en orden cronológico para todo el proyecto (#1–#47) y repartidas
entre las dos carpetas según su tema.

## Estructura del código

```
src/
├── Cargo.toml            # manifiesto del crate: dependencias, perfil release, lints
├── Cargo.lock            # versiones exactas de las dependencias (en el repo: es un binario)
├── rust-toolchain.toml   # fija la toolchain stable + rustfmt/clippy
├── rustfmt.toml          # reglas de formato
├── .cargo/config.toml    # RUST_LOG por defecto para `cargo run`
├── Dockerfile            # build multi-stage -> imagen distroless (operaciones/imagen-de-contenedor.md)
├── .dockerignore
├── k8s/                  # base de Kustomize + overlays local/eks (operaciones/kubernetes.md)
│   ├── monitoring/       # values de Helm, ServiceMonitors, alertas, dashboards de Grafana (operaciones/monitorizacion.md)
│   └── temporal/         # servidor de desarrollo de Temporal, worker y RBAC (operaciones/temporal.md)
├── temporal/             # worker de Temporal en Go: workflow PrescaleForEvent (operaciones/temporal.md)
│   ├── workflow.go       # el workflow: lease, subir mínimo del HPA, esperar pods, mantener, restaurar
│   ├── activities.go     # las activities: llamadas a la API de Kubernetes (lease en anotaciones del HPA)
│   ├── kube.go           # cliente mínimo de la API de Kubernetes (token del ServiceAccount)
│   ├── main.go           # el worker: conexión, registro, métricas en :9090
│   ├── workflow_test.go  # pruebas con el entorno de pruebas de Temporal
│   └── Dockerfile
├── tests/
│   ├── e2e.rs            # pruebas end-to-end contra LocalStack (pruebas.md, task e2e)
│   └── compose.yaml      # entorno de las pruebas: LocalStack + imagen del servicio
└── src/
    ├── main.rs           # arranque: configuración, cliente S3, router, listener, apagado ordenado
    ├── config.rs         # Config::from_env()
    ├── routes.rs         # router, pila de middleware, handlers HTTP
    ├── content.rs        # ContentService: cachés + request coalescing delante de S3
    ├── storage.rs        # S3Store: el único módulo que habla con S3
    ├── state.rs          # AppState compartido por todos los handlers
    ├── telemetry.rs      # tracing + métricas Prometheus + middleware de métricas
    ├── connections.rs    # cuenta las conexiones TCP abiertas (métrica del HPA)
    └── error.rs          # AppError -> mapeo a estado HTTP
```
