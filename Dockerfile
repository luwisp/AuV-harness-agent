# syntax=docker/dockerfile:1

FROM rust:1.88-alpine AS builder

RUN apk add --no-cache musl-dev openssl-dev openssl-libs-static pkgconfig

WORKDIR /app

# Copy only build inputs. Local config, credentials, sessions, and repository
# metadata never enter an image layer.
COPY Cargo.toml Cargo.lock ./
COPY src ./src

RUN cargo build --release --locked \
    && cp target/release/auv /tmp/auv

FROM alpine:3.21 AS runtime

RUN apk add --no-cache \
        bash \
        ca-certificates \
        git \
        libgcc \
        libssl3 \
        openssh-client \
        tini \
    && addgroup -S -g 1000 auv \
    && adduser -S -D -u 1000 -G auv -h /home/auv auv \
    && mkdir -p /home/auv/.AuV /workspace \
    && chown -R auv:auv /home/auv /workspace

COPY --from=builder /tmp/auv /usr/local/bin/auv

ENV HOME=/home/auv \
    XDG_CONFIG_HOME=/home/auv/.AuV
WORKDIR /workspace
USER auv

ENTRYPOINT ["/sbin/tini", "--", "auv"]
