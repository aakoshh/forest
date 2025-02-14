# This Dockerfile is for the main forest binary
# 
# Build and run locally:
# ```
# docker build -t forest:latest -f ./Dockerfile .
# docker run --init -it forest
# ```
# 
# Build and manually push to Github Container Registry (see https://docs.github.com/en/packages/working-with-a-github-packages-registry/working-with-the-container-registry)
# ```
# docker build -t ghcr.io/chainsafe/forest:latest .
# docker push ghcr.io/chainsafe/forest:latest
# ```

##
# Build stage
## 
FROM rust:1-buster AS build-env

# Install dependencies
RUN apt-get update && apt-get install --no-install-recommends -y build-essential clang ocl-icd-opencl-dev cmake

WORKDIR /usr/src/forest
COPY . .

# Grab the correct toolchain
RUN rustup toolchain install nightly && rustup target add wasm32-unknown-unknown

# Install Forest
RUN make install

# strip symbols to make executable smaller
RUN strip /usr/local/cargo/bin/forest

##
# Prod image for forest binary
##
FROM debian:buster-slim

# Link package to the repository
LABEL org.opencontainers.image.source https://github.com/chainsafe/forest

# Install binary dependencies
RUN apt-get update && apt-get install --no-install-recommends -y ocl-icd-opencl-dev libssl1.1 ca-certificates
RUN update-ca-certificates

# Copy forest binary from the build-env
COPY --from=build-env /usr/local/cargo/bin/forest /usr/local/bin/forest

ENTRYPOINT ["/usr/local/bin/forest"]
