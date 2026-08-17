FROM rust:1.95-bookworm AS qpayd
WORKDIR /src
COPY . .
RUN cargo build --release --locked

FROM debian:bookworm-slim AS phoenixd
ARG PHOENIXD_VERSION=0.8.0
ARG PHOENIXD_SHA256=9ffcaf022b2aa676c7b375d5d2c1ead2c529ab4fa6a97986fb72d744d9ab2e92
RUN apt-get update \
  && apt-get install -y --no-install-recommends ca-certificates curl unzip \
  && rm -rf /var/lib/apt/lists/*
ARG PHOENIXD_ASSET_NAME="phoenixd-${PHOENIXD_VERSION}-linux-x64"
RUN curl -fsSL "https://github.com/ACINQ/phoenixd/releases/download/v${PHOENIXD_VERSION}/${PHOENIXD_ASSET_NAME}.zip" \
  -o /tmp/phoenixd.zip \
  && echo "${PHOENIXD_SHA256}  /tmp/phoenixd.zip" | sha256sum -c - \
  && unzip -q /tmp/phoenixd.zip -d /tmp \
  && install -m 755 /tmp/${PHOENIXD_ASSET_NAME}/phoenixd /usr/local/bin/phoenixd \
  && install -m 755 /tmp/${PHOENIXD_ASSET_NAME}/phoenix-cli /usr/local/bin/phoenix-cli

FROM debian:bookworm-slim
RUN apt-get update \
  && apt-get install -y --no-install-recommends ca-certificates curl \
  && rm -rf /var/lib/apt/lists/* \
  && mkdir -p /data
COPY --from=qpayd /src/target/release/qpayd /usr/local/bin/qpayd
COPY --from=phoenixd /usr/local/bin/phoenixd /usr/local/bin/phoenixd
COPY --from=phoenixd /usr/local/bin/phoenix-cli /usr/local/bin/phoenix-cli
COPY deployment/pay-q32/qpayd.toml /etc/qpayd/qpayd.toml
COPY deployment/pay-q32/entrypoint.sh /usr/local/bin/entrypoint.sh
RUN chmod +x /usr/local/bin/entrypoint.sh
EXPOSE 8080
CMD ["entrypoint.sh"]
