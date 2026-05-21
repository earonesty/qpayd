FROM rust:1.95-bookworm AS qpayd
WORKDIR /src
ARG QPAYD_REF=master
RUN git clone https://github.com/earonesty/qpayd.git . \
  && git checkout "$QPAYD_REF" \
  && cargo build --release --locked

FROM debian:bookworm-slim AS barkd
ARG BARKD_VERSION=0.1.4
RUN apt-get update \
  && apt-get install -y --no-install-recommends ca-certificates curl \
  && rm -rf /var/lib/apt/lists/*
RUN curl -fsSL "https://gitlab.com/ark-bitcoin/bark/-/releases/bark-${BARKD_VERSION}/downloads/barkd-${BARKD_VERSION}-linux-x86_64" \
  -o /usr/local/bin/barkd \
  && chmod +x /usr/local/bin/barkd

FROM debian:bookworm-slim
RUN apt-get update \
  && apt-get install -y --no-install-recommends ca-certificates curl \
  && rm -rf /var/lib/apt/lists/* \
  && mkdir -p /data
COPY --from=qpayd /src/target/release/qpayd /usr/local/bin/qpayd
COPY --from=barkd /usr/local/bin/barkd /usr/local/bin/barkd
COPY qpayd.toml /etc/qpayd/qpayd.toml
COPY entrypoint.sh /usr/local/bin/entrypoint.sh
RUN chmod +x /usr/local/bin/entrypoint.sh
EXPOSE 8080
CMD ["entrypoint.sh"]
