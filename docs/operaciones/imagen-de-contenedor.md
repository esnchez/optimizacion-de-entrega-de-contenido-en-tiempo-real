# Imagen de contenedor

Se construye a partir de [`src/Dockerfile`](../../src/Dockerfile) con `src/` como contexto de build.

```bash
cd src
docker build -t content-delivery:0.1.0 .
```

## Estructura del build

```
stage "build"   rust:1-bookworm
  1. copiar Cargo.toml + Cargo.lock, compilar un main.rs ficticio → capa de dependencias (en caché)
  2. copiar src/, compilar el crate real                       → target/release/app

stage "runtime" gcr.io/distroless/cc-debian13:nonroot
  copiar /usr/local/bin/app, ENTRYPOINT ["/usr/local/bin/app"]
```

## Decisiones

| Decisión | Por qué | Contrapartida |
| --- | --- | --- |
| **Build multi-stage** | El toolchain de Rust (~1.5 GB) se queda en la etapa de build; solo se distribuye el binario. Imagen final: **64.4 MB**. | Ninguna digna de mención |
| **Capa de dependencias cacheada por separado** | Las dependencias (aws-sdk-s3 y compañía) tardan ~4 min en compilar con fat LTO. Copiar primero solo `Cargo.toml`/`Cargo.lock` hace que un cambio solo de código reutilice esa capa. | Un truco con un `main.rs` ficticio, necesita un `touch` antes del build real para que cargo detecte el cambio |
| **Runtime distroless `cc-debian13`** | Contiene solo glibc, libgcc y certificados de CA: sin shell, sin gestor de paquetes. El escáner del IDE marcó 3 CVE críticas / 12 altas en `debian:bookworm-slim`. Distroless elimina la mayor parte de esa superficie. | No hay shell en la que hacer `kubectl exec` para depurar. Usa `kubectl debug` con un contenedor efímero en su lugar. |
| **Variante `:nonroot` (uid 65532)** | El proceso nunca se ejecuta como root. El namespace de Kubernetes impone el perfil de Pod Security `restricted`, que lo exige. | No puede escuchar en puertos < 1024 (usamos el 8080) |
| **Certificados de CA incluidos** | El cliente de S3 usa TLS (rustls con las raíces del sistema). Verificado: la imagen completa un handshake TLS con el S3 real de AWS. | — |
| **`.dockerignore` excluye `target/`, `.cargo/`, `k8s/`, `tests/`** | `target/` puede ocupar varios GB; enviarlo como contexto de build haría lento cada build. | — |
| **`rust-toolchain.toml` no se copia a la imagen** | La imagen `rust:1` ya proporciona Rust estable. Copiar el fichero haría que rustup descargase componentes extra (rustfmt, clippy) durante el build. | La versión del compilador de la imagen sigue la etiqueta `rust:1`, no el fichero fijado |
| **La etapa de build se queda en bookworm** | Solo se distribuye la etapa de runtime. Un binario compilado contra glibc 2.36 funciona con la glibc más reciente de Debian 13 (compatible hacia delante). | El IDE sigue informando de CVE en la línea de la etapa de build; no llegan a la imagen final |
| **El `ENTRYPOINT` es el propio binario (PID 1)** | El servicio instala su propio manejador de SIGTERM, así que recibe las señales directamente y drena de forma ordenada. Verificado con `docker stop`. | — |

## Depurar un pod en ejecución sin shell

```bash
kubectl -n cdn debug -it <pod> --image=busybox:1.37 --target=content-delivery
```

## Etiquetado

- `0.1.0`: valor por defecto del manifiesto base (la versión del crate).
- `local`: lo que `kubectl apply -k k8s/overlays/local` espera dentro de minikube.
- `dev-<ID de la imagen>`: etiqueta de cada despliegue local con `task deploy:local`. Depende del
  contenido de la imagen: una imagen nueva cambia la plantilla del pod y dispara un rollout; la
  misma imagen no reinicia nada. El build usa `--provenance=false`: Docker Desktop adjunta metadatos
  de procedencia con la hora del build y, sin eso, el ID cambiaría en cada build.
