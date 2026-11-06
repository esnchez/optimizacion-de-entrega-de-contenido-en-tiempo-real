# Entorno local

Para desarrollar sin Kubernetes: LocalStack (S3) en Docker Compose y el servicio en el host o en su
imagen. El clúster local con minikube está en [kubernetes.md](kubernetes.md); las pruebas que usan
este entorno, en [pruebas.md](pruebas.md#con-task).

## Docker Compose

[`docker-compose.yml`](../../docker-compose.yml) define dos servicios:

- **`localstack`** (4.14.0, solo S3) en el puerto 4566, con healthcheck.
- **`app`**: la imagen del servicio construida desde `content-delivery/`, en el puerto 8089, con TTL
  de caché de 2 s y `MAX_CONNECTION_AGE_SECS=3` para que las pruebas de caducidad y de reciclado
  duren segundos. Va en el perfil `app`: solo la arranca `task e2e`, porque la imagen release (LTO)
  tarda ~10 min en compilarse y `cargo run` unos segundos.

| Tarea | Qué hace |
| --- | --- |
| `task env:up` | Arranca LocalStack |
| `task run` | El servicio en el host contra LocalStack, con TTL cortos (2 s) y edad máxima de conexión de 3 s |
| `task env:seed` | Sube los objetos fijos que usan k6 y la prueba de apagado (`hello.txt`, `videos/segment.bin`, `videos/big.bin`); las pruebas e2e no los necesitan |
| `task env:down` | Para y borra los contenedores, también el del perfil `app` |

No hay paso de inicialización: las pruebas e2e crean el bucket y sus propios objetos.

```bash
task env:up
task run                                         # en otra terminal
curl -i http://localhost:8080/content/hello.txt  # tras `task env:seed`
```

## Otros S3 falsos

Cualquier API compatible con S3 sirve: `S3_ENDPOINT` activa el direccionamiento path-style
([variables](../servicio/configuracion.md#variables-de-entorno)).

**MinIO (Docker):**

```bash
docker run -d -p 9000:9000 -e MINIO_ROOT_USER=minio -e MINIO_ROOT_PASSWORD=minio123 \
  minio/minio server /data
# crea un bucket "content": aws --endpoint-url http://localhost:9000 s3 mb s3://content

cd content-delivery
S3_BUCKET=content S3_ENDPOINT=http://localhost:9000 AWS_REGION=us-east-1 \
AWS_ACCESS_KEY_ID=minio AWS_SECRET_ACCESS_KEY=minio123 cargo run
```

**moto (Python, sin Docker):**

```bash
python -m venv .venv && .venv/bin/pip install "moto[server]" boto3
.venv/bin/moto_server -p 5055 &
python -c "import boto3; s3=boto3.client('s3', endpoint_url='http://127.0.0.1:5055', \
  aws_access_key_id='t', aws_secret_access_key='t', region_name='us-east-1'); \
  s3.create_bucket(Bucket='content'); s3.put_object(Bucket='content', Key='hello.txt', Body=b'hello')"

cd content-delivery
S3_BUCKET=content S3_ENDPOINT=http://127.0.0.1:5055 AWS_REGION=us-east-1 \
AWS_ACCESS_KEY_ID=t AWS_SECRET_ACCESS_KEY=t cargo run
```

## Comandos de desarrollo

```bash
cd content-delivery
cargo build                 # build de depuración
cargo build --release       # build optimizado (LTO, el de la imagen)
cargo test                  # pruebas unitarias (= task test)
cargo clippy --all-targets  # lints (task lint también comprueba el formato)
cargo fmt                   # formato
```

`content-delivery/.cargo/config.toml` fija `RUST_LOG=info,app=debug` por defecto para `cargo run`.

## Windows

Todas las tareas funcionan desde PowerShell o Git Bash: los comandos los ejecuta el intérprete
propio de Task.

- **`~/bin` debe estar en el PATH de PowerShell**, con `minikube`, `kubectl` (1.37) y `helm`. Si no,
  `kubectl` resuelve al 1.32 de Docker Desktop, cinco versiones por detrás del clúster.
- **`curl.exe` de Windows no puede escribir en `/dev/null`** (código 23); dentro de Task la
  redirección `> /dev/null` sí funciona.
- **Go exige LF** (`gofmt`) aunque el resto del repositorio use CRLF.
