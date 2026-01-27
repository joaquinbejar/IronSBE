# =============================================================================
# IronSBE Server Dockerfile
# =============================================================================
# Multi-stage build for minimal production image using Alpine Linux
#
# Build:
#   docker build -t ironsbe-server -f Docker/Dockerfile .
#
# Run:
#   docker run -p 9000:9000 ironsbe-server
#
# =============================================================================

# -----------------------------------------------------------------------------
# Stage 1: Builder
# -----------------------------------------------------------------------------
FROM rust:1.92.0-alpine3.23 AS builder

# Install build dependencies for Alpine
RUN apk add --no-cache \
    musl-dev \
    pkgconfig \
    build-base

# Create app directory
WORKDIR /app

# Copy workspace files
COPY Cargo.toml Cargo.lock ./
COPY rust-toolchain.toml ./

# Copy all crate directories
COPY ironsbe ./ironsbe
COPY ironsbe-core ./ironsbe-core
COPY ironsbe-schema ./ironsbe-schema
COPY ironsbe-codegen ./ironsbe-codegen
COPY ironsbe-derive ./ironsbe-derive
COPY ironsbe-channel ./ironsbe-channel
COPY ironsbe-transport ./ironsbe-transport
COPY ironsbe-server ./ironsbe-server
COPY ironsbe-client ./ironsbe-client
COPY ironsbe-marketdata ./ironsbe-marketdata
COPY ironsbe-bench ./ironsbe-bench

# Build the server example in release mode
RUN cargo build --release --example server -p ironsbe

# -----------------------------------------------------------------------------
# Stage 2: Runtime
# -----------------------------------------------------------------------------
FROM alpine:3.23 AS runtime

# Install runtime dependencies
RUN apk add --no-cache \
    ca-certificates \
    netcat-openbsd

# Create non-root user for security
RUN adduser -D -u 1000 ironsbe

# Create app directory
WORKDIR /app

# Copy the built binary from builder stage
COPY --from=builder /app/target/release/examples/server /app/ironsbe-server

# Change ownership to non-root user
RUN chown -R ironsbe:ironsbe /app

# Switch to non-root user
USER ironsbe

# Expose the default SBE server port
EXPOSE 9000

# Set environment variables
ENV RUST_LOG=info
ENV RUST_BACKTRACE=1

# Health check
HEALTHCHECK --interval=30s --timeout=3s --start-period=5s --retries=3 \
    CMD nc -z localhost 9000 || exit 1

# Run the server
ENTRYPOINT ["/app/ironsbe-server"]
