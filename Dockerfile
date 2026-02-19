# syntax=docker/dockerfile:1.7

FROM rust:1-bookworm AS builder
WORKDIR /src

RUN apt-get update \
    && apt-get install -y --no-install-recommends \
       build-essential \
       cmake \
       ninja-build \
       perl \
       pkg-config \
       clang \
       libclang-dev \
       llvm-dev \
    && rm -rf /var/lib/apt/lists/*

# 1) 仅复制依赖清单，用空壳 crate 预编译依赖（缓存层）
COPY Cargo.toml Cargo.lock ./
RUN mkdir src && echo "fn main() {}" > src/main.rs \
    && LIBCLANG_PATH="$(llvm-config --libdir)" cargo build --release \
    && rm -rf src

# 2) 复制真实源码及编译期嵌入的资源目录，仅增量编译业务代码
COPY static/ static/
COPY src/ src/
RUN touch src/main.rs \
    && LIBCLANG_PATH="$(llvm-config --libdir)" cargo build --release \
    && test "$(stat -c%s target/release/grok2api-rs)" -gt 5000000

FROM debian:bookworm-slim AS runtime
WORKDIR /app

RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates tzdata \
    && rm -rf /var/lib/apt/lists/* \
    && useradd --create-home --uid 10001 appuser

COPY --from=builder /src/target/release/grok2api-rs /app/grok2api-rs
COPY config.defaults.toml /app/config.defaults.toml
COPY docker/entrypoint.sh /app/entrypoint.sh

RUN apt-get update \
    && apt-get install -y --no-install-recommends gosu \
    && rm -rf /var/lib/apt/lists/* \
    && sed -i 's/\r$//' /app/entrypoint.sh \
    && chmod +x /app/grok2api-rs /app/entrypoint.sh \
    && mkdir -p /app/data \
    && chown -R appuser:appuser /app
EXPOSE 8000

ENV SERVER_HOST=0.0.0.0 \
    SERVER_PORT=8000

ENTRYPOINT ["/app/entrypoint.sh"]
CMD ["/app/grok2api-rs"]
