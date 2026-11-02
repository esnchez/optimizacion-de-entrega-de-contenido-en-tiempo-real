# Imagen del worker de Temporal. Multi-stage: compila con la toolchain de Go y copia solo el
# binario estático a una imagen distroless sin shell, como la del servicio de contenido.
FROM golang:1.27-bookworm AS build
WORKDIR /src

# Primero solo las dependencias, para que Docker las cachee entre builds.
COPY go.mod go.sum ./
RUN go mod download

COPY *.go ./
# CGO_ENABLED=0: binario estático, sin depender de la libc de la imagen final.
RUN CGO_ENABLED=0 go build -trimpath -ldflags="-s -w" -o /worker .

FROM gcr.io/distroless/static-debian13:nonroot
COPY --from=build /worker /usr/local/bin/worker
EXPOSE 9090
ENTRYPOINT ["/usr/local/bin/worker"]
