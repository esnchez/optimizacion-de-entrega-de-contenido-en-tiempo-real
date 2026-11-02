# Configuración

Toda la configuración viene de variables de entorno (estilo 12-factor, así se corresponde
directamente con un `ConfigMap`/`env` de Kubernetes). Los valores no válidos detienen el servicio
al arrancar con un error claro.

En Kubernetes los valores están en el `configMapGenerator` de `src/k8s/base/kustomization.yaml`,
con valores específicos por entorno en los overlays ([kubernetes.md](../operaciones/kubernetes.md#estructura)).
`MAX_CONCURRENT_REQUESTS`, `STREAM_CHUNK_BYTES` y `BODY_CACHE_MAX_BYTES` determinan la memoria del
pod en el peor caso y deben ser coherentes con su límite de memoria.

## Variables de entorno

| Variable | Valor por defecto | Descripción |
| --- | --- | --- |
| `S3_BUCKET` | **obligatoria** | Bucket que contiene el contenido |
| `AWS_REGION` | cadena de proveedores | Región de AWS |
| `S3_ENDPOINT` | — | Endpoint personalizado (MinIO, LocalStack, moto). Activa el direccionamiento path-style. |
| `BIND_ADDR` | `0.0.0.0:8080` | Dirección de escucha |
| `MAX_CONCURRENT_REQUESTS` | `10000` | Peticiones de contenido en curso por pod antes del load shedding (`503`). En Kubernetes, 4 000 (`k8s/base`); pendiente de revisar con la capacidad medida por pod ([estado](../estado.md)) |
| `REQUEST_TIMEOUT_MS` | `10000` | Tiempo límite para producir las cabeceras de la respuesta (`504`) |
| `METADATA_CACHE_TTL_SECS` | `60` | Cuánto tiempo se guardan en caché los metadatos de los objetos (y los "no encontrado") |
| `METADATA_CACHE_CAPACITY` | `100000` | Número máximo de entradas de metadatos |
| `SMALL_OBJECT_MAX_BYTES` | `1048576` (1 MiB) | Los objetos de hasta este tamaño se sirven desde memoria |
| `BODY_CACHE_MAX_BYTES` | `268435456` (256 MiB) | Presupuesto de memoria para los cuerpos en caché |
| `BODY_CACHE_TTL_SECS` | `60` | Cuánto tiempo viven los cuerpos en caché |
| `PRESIGN_TTL_SECS` | `300` | Vigencia de las URL de `/url/{key}` |
| `STREAM_CHUNK_BYTES` | `65536` | Búfer de lectura por respuesta en streaming |
| `CACHE_CONTROL` | `public, max-age=300` | Cabecera `Cache-Control` del contenido |
| `SHUTDOWN_FLUSH_SECS` | `5` | Espera tras el drenado antes de salir, para que los datos que quedan en los búferes del socket lleguen a los clientes |
| `MAX_CONNECTION_AGE_SECS` | `300` | Edad máxima de una conexión (menos 0–20 % aleatorio por conexión). Pasada, la siguiente respuesta HTTP/1.1 lleva `Connection: close` y el cliente se reconecta, posiblemente a otro pod. Reparte las conexiones tras escalar. `0` lo desactiva. |
| `RUST_LOG` | `info` | Filtro de logs, p. ej. `info,app=debug` |
| `LOG_FORMAT` | texto | `json` para logs estructurados |

Las credenciales de AWS siguen la cadena estándar: `AWS_ACCESS_KEY_ID`/`AWS_SECRET_ACCESS_KEY`,
`AWS_PROFILE`, IRSA en EKS (recomendado en producción) o los metadatos de la instancia.

La política IAM solo necesita `s3:GetObject` sobre los objetos del bucket. Añade `s3:ListBucket`
sobre el bucket si quieres que las claves inexistentes devuelvan `404` en lugar de `403` desde S3;
si no, aparecen como `502`.

## Ejecución en local contra un S3 falso

### Con MinIO (Docker)

```bash
docker run -d -p 9000:9000 -e MINIO_ROOT_USER=minio -e MINIO_ROOT_PASSWORD=minio123 \
  minio/minio server /data

# crea un bucket "content" y sube algo con la consola de MinIO (http://localhost:9000)
# o con: aws --endpoint-url http://localhost:9000 s3 mb s3://content

cd src
S3_BUCKET=content S3_ENDPOINT=http://localhost:9000 AWS_REGION=us-east-1 \
AWS_ACCESS_KEY_ID=minio AWS_SECRET_ACCESS_KEY=minio123 cargo run
```

### Con moto (Python, sin Docker)

```bash
python -m venv .venv && .venv/bin/pip install "moto[server]" boto3
.venv/bin/moto_server -p 5055 &
python -c "import boto3; s3=boto3.client('s3', endpoint_url='http://127.0.0.1:5055', \
  aws_access_key_id='t', aws_secret_access_key='t', region_name='us-east-1'); \
  s3.create_bucket(Bucket='content'); s3.put_object(Bucket='content', Key='hello.txt', Body=b'hello')"

cd src
S3_BUCKET=content S3_ENDPOINT=http://127.0.0.1:5055 AWS_REGION=us-east-1 \
AWS_ACCESS_KEY_ID=t AWS_SECRET_ACCESS_KEY=t cargo run
curl -i http://localhost:8080/content/hello.txt
```

## Con LocalStack y Docker

Consulta [pruebas.md](../pruebas.md#entorno-docker-compose): ejecuta la imagen del servicio y LocalStack en una misma
red de Docker, y cubre también las pruebas end-to-end y de carga.

## Comandos de desarrollo

```bash
cd src
cargo build                 # build de depuración
cargo build --release       # build optimizado (LTO, usado en las pruebas de carga)
cargo test                  # pruebas unitarias (= task test)
cargo clippy --all-targets  # lints (task lint también comprueba el formato)
cargo fmt                   # formato
```

Para ejecutar el servicio en local contra LocalStack sin escribir las variables de entorno:
`task env:up` (LocalStack en Docker) y `task run` (servicio en el host con TTL cortos).
