FROM rust:bookworm

WORKDIR /grcov

RUN apt-get update && apt-get install -y --no-install-recommends g++-11 clang-13 &&  rm -rf /var/lib/apt/lists/*
RUN rustup component add llvm-tools

# Fetch and build dependencies in a cachable step (hack until https://github.com/rust-lang/cargo/issues/2644 is fixed).
COPY Cargo.toml Cargo.lock ./
RUN mkdir src/ && echo "fn main() {}" > src/main.rs && cargo build && cargo clean -p grcov && rm -r src/

COPY . .

RUN cargo build

ENV CLANG_CXX=clang++-13
ENV GCC_CXX=g++-11
ENV GCOV=gcov-11

RUN cargo test
