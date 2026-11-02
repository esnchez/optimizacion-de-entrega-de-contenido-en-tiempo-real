# Optimización de Entrega de Contenido en Tiempo Real

**Problema:** Los sistemas de streaming deben entregar contenido sin interrupciones y con latencia mínima a millones de usuarios simultáneamente, lo que genera sobrecarga en servidores y costos elevados.

**Solución:** Construí un sistema de entrega de contenido en tiempo real optimizado para reducir la latencia y costos operativos usando Kubernetes y Rust.

**Impacto:** Manejó picos de 5M de conexiones simultáneas, redujo la latencia de 300ms a 50ms, y ahorró $100K al mes en costos de infraestructura.

**Tecnologías:** Rust, Kubernetes, AWS S3, Temporal

**Tecnologías nuevas a aprender:** Rust, Kubernetes, Temporal


## Plan por fases

1. Fase 1: Diseño de arquitectura y definición de métricas de rendimiento
2. Fase 2: Implementación del sistema en Rust con gestión de eventos en Temporal
3. Fase 3: Despliegue en Kubernetes y optimización de latencia
4. Fase 4: Monitoreo y ajuste continuo, recopilación de métricas de uso

**Estado de cada fase y de cada objetivo, con lo hecho y lo pendiente: [docs/estado.md](docs/estado.md).**
Los números de *Impacto* son los objetivos del proyecto, no resultados medidos.

## Explicación

Este proyecto consiste en la creación de un sistema de entrega de contenido en tiempo real, diseñado para optimizar la latencia y reducir los costos operativos en sistemas de streaming que manejan millones de conexiones simultáneas. Utilizaremos Kubernetes para la orquestación de contenedores, Rust para el backend debido a su excelente rendimiento y seguridad en concurrencia, y Temporal para la orquestación de flujos de trabajo. El objetivo es manejar picos de hasta 5 millones de conexiones simultáneas, reducir la latencia de 300ms a 50ms, y ahorrar significativamente en costos de infraestructura.

A nivel alto, el sistema utiliza Kubernetes para escalar horizontalmente los servicios en función de la carga, aprovechando la capacidad de Rust para manejar tareas concurrentes de manera eficiente. Temporal se emplea para gestionar procesos complejos y mantener la resiliencia del sistema. AWS S3 se utiliza como almacenamiento de objetos para los contenidos a distribuir.

Este proyecto enseña conceptos clave como la escalabilidad horizontal, la optimización de la latencia en tiempo real, y la integración de tecnologías de orquestación como Kubernetes y Temporal. Rust se introduce como un lenguaje de alto rendimiento ideal para sistemas concurrentes, mientras que Kubernetes y Temporal ofrecen experiencia en la gestión y orquestación de sistemas distribuidos.

## Arquitectura

```
| Usuario |
    |
    v
| Ingress Controller (Nginx) |
    |
    v
| Kubernetes Cluster |
    |
    +--------------------+
    |                    |
    v                    v
| Rust Microservice |  | Temporal Workflow |
    |                    |
    v                    v
| AWS S3 (Content) |  | AWS S3 (Logs)    |
```
- **Usuario**: Cliente que solicita contenido.
- **Ingress Controller**: Gestiona el tráfico entrante al clúster de Kubernetes.
- **Kubernetes Cluster**: Orquesta los contenedores de servicios de Rust y Temporal.
- **Rust Microservice**: Procesa solicitudes de contenido, optimizando la entrega.
- **Temporal Workflow**: Orquesta los flujos de trabajo y procesos de negocio.
- **AWS S3**: Almacena contenido y logs para análisis posterior.

Estado y matices respecto al diagrama:

- **Temporal queda fuera del camino de cada petición** (un workflow tarda de milisegundos a segundos, incompatible con p99 < 50 ms). Se usa para trabajo lento en segundo plano. Primer workflow: `PrescaleForEvent`, que sube el mínimo del HPA antes de un evento conocido y lo restaura después ([docs/temporal.md](docs/operaciones/temporal.md)).
- **Conexiones simultáneas y balanceador:** en local se entra por ingress-nginx. Para escalar por conexiones simultáneas en producción, las conexiones deben llegar a los pods a través de un balanceador L4 (NLB): un proxy L7 (nginx, ALB) es quien sostendría los 5M de conexiones y los pods solo verían su pool keep-alive ([docs/kubernetes.md](docs/operaciones/kubernetes.md)).
- **Logs a S3:** la caja "AWS S3 (Logs)" del diagrama aún no existe; es un workflow de Temporal candidato ([docs/operaciones/temporal.md](docs/operaciones/temporal.md#otros-workflows-candidatos)).
- **Observabilidad:** Prometheus + Grafana dentro del clúster en local; en producción, fuera del clúster con Amazon Managed Prometheus y Amazon Managed Grafana ([docs/monitorizacion.md](docs/operaciones/monitorizacion.md#monitorización-en-producción)).

## Cómo empezar

Requiere Docker, minikube, kubectl, Helm, Rust, Go y [Task](https://taskfile.dev) 3.x. `task` lista
todas las tareas del [`Taskfile.yml`](Taskfile.yml); funcionan desde PowerShell, Git Bash, Linux o macOS.

```bash
# Pruebas, sin clúster
task test               # unitarias
task e2e                # end-to-end con Docker Compose (LocalStack + imagen del servicio)
task temporal:test      # workflow de Temporal

# Clúster local (minikube + LocalStack como S3)
minikube start --driver=docker --cpus=4 --memory=6g --addons=ingress,metrics-server
task deploy:local
task monitoring:install
task grafana            # http://localhost:3000 (admin / admin)
task temporal:deploy    # opcional: servidor de Temporal y worker
task temporal:ui        # http://localhost:8233

# Pruebas en el clúster
task e2e:cluster        # con `task cluster:forward` en otra terminal
task load:capacity      # capacidad de un pod
task load:scaling       # el HPA escala por conexiones
```

La documentación está en [docs/](docs/README.md), agrupada en [servicio](docs/servicio) (el código
en Rust) y [operaciones](docs/operaciones) (despliegue, monitorización y Temporal).

## Despliegue a cloud

El overlay de EKS está preparado pero sin desplegar: `task deploy:eks` se niega a aplicarlo mientras
queden valores `<...>` (bucket, rol IRSA, imagen en ECR). Lo que falta para producción está en
[docs/estado.md](docs/estado.md#pendiente).

## Pruebas de carga

k6 como Job dentro del clúster, con sus métricas en Prometheus y Grafana. 5M de conexiones no caben en
un portátil: se mide la capacidad de un pod y se extrapola. Procedimiento, resultados y conclusiones
en [docs/pruebas.md](docs/pruebas.md).

## Métricas objetivo

"5M" se refiere a **conexiones simultáneas** (usuarios conectados a la vez), no a peticiones por
segundo. Con un segmento de vídeo cada ~6 s por espectador, equivalen a unas 830 000 peticiones/s en
toda la flota.

| Métrica | Objetivo | Cómo medir |
| --- | --- | --- |
| Conexiones simultáneas | 5.000.000 en toda la flota | `http_connections_open` por pod × pods; prueba de capacidad por pod con el perfil `connections` de k6 |
| Latencia p50 / p95 / p99 | p99 < 50ms | `http_request_duration_seconds` (servidor) y k6 (cliente), en Grafana |
| Tasa de error | < 0.1% | `http_requests_total` por estado y k6 (`http_req_failed`) |
| Punto de saturación | Conexiones por pod a partir de las que sube la latencia o la memoria se acerca al límite | Escalones de la prueba de capacidad por pod |

El resultado de cada métrica está en [docs/estado.md](docs/estado.md#objetivos-del-readme).

## Pruebas (unit / integración)

Unitarias en Rust, end-to-end en Rust contra LocalStack (18 pruebas, en Docker Compose, en el host o
en el clúster) y del workflow de Temporal en Go. Comandos en [Cómo empezar](#cómo-empezar); detalle
en [docs/pruebas.md](docs/pruebas.md).

## Complejidades técnicas

### Escalabilidad Horizontal

**Por qué es difícil:** Manejar millones de conexiones concurrentes requiere escalabilidad eficiente.

**Cómo se resuelve:** Usando Kubernetes para orquestar y escalar servicios automáticamente.

### Baja Latencia

**Por qué es difícil:** Es crucial para la experiencia de usuario en streaming.

**Cómo se resuelve:** Rust se utiliza por su alto rendimiento y concurrencia eficiente.

### Orquestación de Flujos de Trabajo

**Por qué es difícil:** Coordinar múltiples procesos en tiempo real es complejo.

**Cómo se resuelve:** Temporal maneja la orquestación de manera resiliente.


## Takeaways

Qué se ha conseguido y qué falta frente a estos objetivos: [docs/estado.md](docs/estado.md).

## Ruta de aprendizaje

### Rust

Ofrece alto rendimiento y seguridad en concurrencia ideal para sistemas de baja latencia.

1. Aprende los fundamentos de Rust: variables, control de flujo, funciones.
2. Profundiza en concurrencia y seguridad: ownership, borrowing.
3. Aplica a microservicios: construye un servicio básico con Actix o Tokio.
4. Integra con Kubernetes: despliega el servicio en un clúster local.


- The Rust Programming Language (libro)
- Rust by Example (sitio web)

### Kubernetes

Es esencial para la orquestación y escalabilidad de aplicaciones en la nube.

1. Aprende los conceptos básicos: Pods, Deployments, Services.
2. Configura un clúster local con Minikube: despliega una aplicación simple.
3. Gestiona la escalabilidad: ajusta réplicas y recursos.
4. Integra con Rust: despliega un microservicio Rust en Kubernetes.


- Kubernetes Official Documentation
- Kubernetes Up & Running (libro)

### Temporal

Facilita la gestión de flujos de trabajo resilientes y escalables.

1. Comprende los fundamentos: workflows, activities.
2. Configura un entorno de desarrollo: ejecuta ejemplos básicos.
3. Desarrolla un flujo de trabajo simple: integra con un microservicio.
4. Escala y monitorea: observa el comportamiento en un entorno distribuido.


- Temporal Documentation
- Temporal's Hello World Tutorial
