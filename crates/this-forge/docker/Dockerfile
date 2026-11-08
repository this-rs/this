# Image de build de la forge : toolchain + cible wasm32 + dépendances VENDORÉES.
# Construite AVEC réseau (une fois) ; exécutée ensuite SANS réseau.
#   docker build -t this-forge-builder:1.93 -f docker/Dockerfile docker
FROM rust:1.93-slim
RUN rustup target add wasm32-unknown-unknown
COPY template /forge/template
WORKDIR /forge/template
# Vendoring : seul ce qui est dans template/Cargo.toml (+ Cargo.lock) existe hors ligne.
RUN cargo vendor --locked /forge/vendor > /usr/local/cargo/config.toml \
 && chmod -R a+rX /forge /usr/local/cargo/config.toml
