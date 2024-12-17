mod? local

# An alias for cargo +nightly fmt --all
fmt *args:
    cargo +nightly fmt --all {{args}}

lint *args:
    cargo +nightly clippy --fix --allow-dirty --all-targets --all-features --allow-staged {{args}}

test *args:
    #!/bin/bash
    set -euo pipefail

    # We use the nightly toolchain for coverage since it supports branch & no-coverage flags.
    export RUSTUP_TOOLCHAIN=nightly

    INSTA_FORCE_PASS=1 cargo llvm-cov clean --workspace
    INSTA_FORCE_PASS=1 cargo llvm-cov nextest --branch --include-build-script --no-report {{args}}

    # Do not generate the coverage report on CI
    cargo insta review
    cargo llvm-cov report --html
    cargo llvm-cov report --lcov --output-path ./lcov.info

deny *args:
    cargo deny {{args}} --all-features check

generate:
    RUSTUP_TOOLCHAIN=nightly sea-orm-cli generate entity -o server/src/entities

migrate-sqlite *args:
    sea-orm-cli migrate {{args}} --database-url sqlite:./brawl.db

migrate-postgres *args:
    sea-orm-cli migrate {{args}} --database-url postgres://brawl:brawl@localhost:5432/brawl

generate-entities *args:
    RUSTUP_TOOLCHAIN=nightly sea-orm-cli generate entity -o server/src/entities {{args}}

workspace-hack:
    cargo hakari manage-deps
    cargo hakari generate
