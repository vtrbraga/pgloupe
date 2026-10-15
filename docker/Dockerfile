# The image is built from the release binaries, not from source: the build context holds
# amd64/pgloupe and arm64/pgloupe, taken from the linux-gnu archives of the release.
FROM debian:bookworm-slim

ARG VERSION=unknown
LABEL org.opencontainers.image.title="pgloupe" \
      org.opencontainers.image.description="A close look at your Postgres logs and query stats." \
      org.opencontainers.image.source="https://github.com/vtrbraga/pgloupe" \
      org.opencontainers.image.licenses="MIT OR Apache-2.0" \
      org.opencontainers.image.version="${VERSION}"

# CA certificates, for TLS to a server (snap, doctor, watch).
RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates \
    && rm -rf /var/lib/apt/lists/* \
    && useradd --uid 10001 --create-home pgloupe

ARG TARGETARCH
COPY ${TARGETARCH}/pgloupe /usr/local/bin/pgloupe

USER pgloupe
WORKDIR /data
ENTRYPOINT ["pgloupe"]
CMD ["--help"]
