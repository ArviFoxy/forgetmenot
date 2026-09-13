# The one environment the screenshots are recorded and compared in. The image tag
# has to match the @playwright/test version in web/package.json, because the browsers
# that ship in the image are the ones that version drives.
FROM mcr.microsoft.com/playwright:v1.63.0-noble

# The tests start the real server, so the image can build it: a C toolchain for the
# bundled sqlite and libgit2, and the Rust toolchain the workspace asks for.
RUN apt-get update \
    && apt-get install -y --no-install-recommends \
        build-essential \
        ca-certificates \
        cmake \
        curl \
        git \
        pkg-config \
    && rm -rf /var/lib/apt/lists/*

# Toolchain and caches live outside the home directory and are writable by everyone,
# because the container runs as the user that owns the mounted repository.
ENV RUSTUP_HOME=/opt/rustup \
    CARGO_HOME=/opt/cargo \
    PATH=/opt/cargo/bin:$PATH \
    CARGO_TARGET_DIR=/cargo-target

ARG RUST_VERSION=1.98.1
RUN curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs \
        | sh -s -- -y --profile minimal --default-toolchain ${RUST_VERSION} \
    && mkdir -p /opt/cargo/registry /opt/cargo/git /cargo-target \
    && chmod -R a+rwX /opt/rustup /opt/cargo /cargo-target

# npm and Playwright write here, the run's user having no home directory of its own.
ENV HOME=/tmp \
    npm_config_cache=/tmp/npm-cache

COPY entrypoint.sh /usr/local/bin/entrypoint.sh
ENTRYPOINT ["/usr/local/bin/entrypoint.sh"]
WORKDIR /workspace
