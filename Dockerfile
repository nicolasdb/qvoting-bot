# ===== BMO's Rust Discord Bot Container Recipe =====
# Multi-stage build for smaller final image

# Build stage
FROM rust:latest as builder

WORKDIR /app

# Copy manifests first for better caching
COPY Cargo.toml ./

# Create a dummy main.rs to build dependencies
RUN mkdir src && echo "fn main() {}" > src/main.rs

# Build dependencies (cached layer) - this generates Cargo.lock
RUN cargo build --release

# Remove dummy source
RUN rm -rf src/

# Copy real source code
COPY src ./src

# Build the actual application
RUN touch src/main.rs && cargo build --release

# Runtime stage - smaller alpine image
FROM debian:bookworm-slim

# Install SSL certificates for HTTPS connections to Discord
RUN apt-get update && \
    apt-get install -y ca-certificates && \
    rm -rf /var/lib/apt/lists/*

# Create non-root user for security
RUN useradd -m -s /bin/bash qvoting

WORKDIR /app

# Copy the binary from builder stage
COPY --from=builder /app/target/release/qvoting-bot ./

# Change ownership to non-root user
RUN chown -R qvoting:qvoting /app
USER qvoting

# Expose port (not really needed for Discord bots, but good practice)
EXPOSE 8080

# Run the bot
CMD ["./qvoting-bot"]
