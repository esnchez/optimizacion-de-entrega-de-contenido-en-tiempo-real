# Dockerfile para construir la imagen del microservicio Rust.
# Multi-stage: compila con la toolchain completa y copia solo el binario a una imagen mínima.

FROM rust:1-bookworm AS build
WORKDIR /usr/src/app

# Compila primero solo las dependencias para que Docker las cachee entre builds.
COPY Cargo.toml Cargo.lock ./
RUN mkdir src && echo 'fn main() {}' > src/main.rs \
    && cargo build --release \
    && rm -rf src

COPY src ./src
# touch: fuerza a cargo a recompilar el crate en lugar de reutilizar el main.rs vacío.
RUN touch src/main.rs && cargo build --release

# Distroless: solo glibc, libgcc y ca-certificates (TLS hacia S3). Sin shell ni gestor de
# paquetes, lo que reduce la superficie de ataque y las CVEs. Usuario no root por defecto.
FROM gcr.io/distroless/cc-debian13:nonroot
COPY --from=build /usr/src/app/target/release/app /usr/local/bin/app
EXPOSE 8080
ENTRYPOINT ["/usr/local/bin/app"]
