# Build Stage
FROM rust:1.96-alpine AS builder
WORKDIR /usr/src/
# Install required build dependencies
RUN apk add --no-cache musl-dev pkgconfig gcc g++ make

# - Install dependencies
WORKDIR /usr/src
RUN USER=root cargo new paperless-mcp
WORKDIR /usr/src/paperless-mcp
COPY Cargo.toml Cargo.lock ./
RUN cargo build --release

# - Copy source
COPY src ./src
RUN touch src/main.rs && cargo build --release

# ---- Runtime Stage ----
FROM alpine:latest AS runtime
# pdftoppm renders document pages for get_document_pages
RUN apk add --no-cache poppler-utils
COPY --from=builder /usr/src/paperless-mcp/target/release/paperless-mcp /usr/local/bin/paperless-mcp
USER 1000
CMD ["paperless-mcp"]
