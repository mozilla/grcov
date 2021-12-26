FROM rust:buster

WORKDIR /grcov

RUN apt-get update && apt-get install -y --no-install-recommends g++-7 clang-7 &&  rm -rf /var/lib/apt/lists/*

# Fetch and build dependencies in a cachable step (hack until https://github.com/rust-lang/cargo/issues/2644 is fixed).
COPY Cargo.toml Cargo.lock ./
RUN mkdir src/ && echo "fn main() {}" > src/main.rs && cargo build && rm target/debug/deps/grcov* && rm -r src/

COPY . .

RUN cargo build

ENV CLANG_CXX=clang++-7
ENV GCC_CXX=g++-7
ENV GCOV=gcov-7

RUN cargo test
