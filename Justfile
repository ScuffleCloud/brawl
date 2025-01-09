mod? local

set shell := ["/bin/bash", "-euo", "pipefail", "-uc"]

# By default we use the nightly toolchain, however you can override this by setting the RUST_TOOLCHAIN environment variable.
export RUST_TOOLCHAIN := env_var_or_default('RUST_TOOLCHAIN', 'nightly')

# Runs cargo fmt
fmt *args:
    cargo +{{RUST_TOOLCHAIN}} fmt --all {{args}}

# Runs cargo clippy
lint *args:
    cargo +{{RUST_TOOLCHAIN}} clippy --fix --allow-dirty --all-targets --all-features --allow-staged {{args}}

# Runs cargo test
test *args:
    #!/bin/bash
    set -euo pipefail

    INSTA_FORCE_PASS=1 cargo +{{RUST_TOOLCHAIN}} llvm-cov clean --workspace
    INSTA_FORCE_PASS=1 cargo +{{RUST_TOOLCHAIN}} llvm-cov nextest --include-build-script --no-report -- {{args}}
    # Coverage for doctests is currently broken in llvm-cov.
    # Once it fully works we can add the `--doctests` flag to the test and report command again.
    cargo +{{RUST_TOOLCHAIN}} llvm-cov test --doc --no-report -- {{args}}

    # Do not generate the coverage report on CI
    cargo insta review
    cargo +{{RUST_TOOLCHAIN}} llvm-cov report --lcov --output-path ./lcov.info
    cargo +{{RUST_TOOLCHAIN}} llvm-cov report --html

# Runs cargo deny
deny *args:
    cargo +{{RUST_TOOLCHAIN}} deny {{args}} --all-features check

# Update the workspace dependencies
workspace-hack:
    cargo +{{RUST_TOOLCHAIN}} hakari manage-deps
    cargo +{{RUST_TOOLCHAIN}} hakari generate

schema_path := "server/src/database/schema.rs"

# Generate the schema file
diesel-generate: _diesel-generate-unpatched
	touch migrations/schema.patch
	cp migrations/schema.unpatched.rs {{schema_path}}
	just diesel-apply

# Generate the patch file
diesel-patch: _diesel-generate-unpatched
	[ -s {{schema_path}} ] || cp migrations/schema.unpatched.rs {{schema_path}}
	diff -U6 migrations/schema.unpatched.rs {{schema_path}} > migrations/schema.patch || true

# Apply the patch file to the schema file
diesel-apply:
	[ ! -s migrations/schema.patch ] || patch -p0 -o {{schema_path}} --merge < migrations/schema.patch

# Check if the generated schema is up-to-date
diesel-check:
	@ \
		check=$(just _diesel-generate-unpatched-helper 2> /dev/null) && \
		diff -q <(echo "$check") migrations/schema.unpatched.rs > /dev/null || ( \
			echo "The generated schema differs from {{schema_path}}. Run 'just diesel-generate'."; \
			exit 1; \
		)

	@ \
		regex='s/^\(\(\+\+\+\|\-\-\-\)[^\t]*\)\t.*$/\1\t<timestamp>/' && \
		check=$(diff -U6 migrations/schema.unpatched.rs {{schema_path}} | sed "$regex" || echo '') && \
		patch=$(sed "$regex" ./migrations/schema.patch) && \
		diff -q <(echo "$check") <(echo "$patch") > /dev/null || ( \
			echo "The patch file differs from what would be generated. Run 'just diesel-patch'."; \
			exit 1; \
		);

	@echo "Diesel schema and patch are up-to-date!"

_diesel-generate-unpatched:
	just _diesel-generate-unpatched-helper > migrations/schema.unpatched.rs

_diesel-generate-unpatched-helper:
	diesel print-schema --patch-file=<(echo '')

docker-build TAG='scuffle-brawl:latest':
	docker build -t {{TAG}} -f server/Dockerfile .
