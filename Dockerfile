FROM rust:1.85-alpine AS builder
RUN apk add --no-cache musl-dev pkgconfig openssl-dev
WORKDIR /app
COPY . .
RUN cargo build --release

FROM alpine:3.21
RUN apk add --no-cache ca-certificates git
COPY --from=builder /app/target/release/harness /usr/local/bin/harness
ENTRYPOINT ["harness"]