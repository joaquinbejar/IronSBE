# Makefile for common tasks in a Rust project
# Detect current branch
CURRENT_BRANCH := $(shell git rev-parse --abbrev-ref HEAD)
ZIP_NAME = ironsbe.zip

# Set version across all crates
# Usage: make version VERSION=0.1.1
.PHONY: version
version:
	@if [ -z "$(VERSION)" ]; then echo "Usage: make version VERSION=x.y.z"; exit 1; fi
	@echo "Setting version to $(VERSION) across all crates..."
	@sed -i '' 's/^version = "[^"]*"/version = "$(VERSION)"/' Cargo.toml
	@sed -i '' 's/ironsbe-core = { path = "ironsbe-core", version = "[^"]*"/ironsbe-core = { path = "ironsbe-core", version = "$(VERSION)"/' Cargo.toml
	@sed -i '' 's/ironsbe-dictionary = { path = "ironsbe-dictionary", version = "[^"]*"/ironsbe-dictionary = { path = "ironsbe-dictionary", version = "$(VERSION)"/' Cargo.toml
	@sed -i '' 's/ironsbe-tagvalue = { path = "ironsbe-tagvalue", version = "[^"]*"/ironsbe-tagvalue = { path = "ironsbe-tagvalue", version = "$(VERSION)"/' Cargo.toml
	@sed -i '' 's/ironsbe-session = { path = "ironsbe-session", version = "[^"]*"/ironsbe-session = { path = "ironsbe-session", version = "$(VERSION)"/' Cargo.toml
	@sed -i '' 's/ironsbe-store = { path = "ironsbe-store", version = "[^"]*"/ironsbe-store = { path = "ironsbe-store", version = "$(VERSION)"/' Cargo.toml
	@sed -i '' 's/ironsbe-transport = { path = "ironsbe-transport", version = "[^"]*"/ironsbe-transport = { path = "ironsbe-transport", version = "$(VERSION)"/' Cargo.toml
	@sed -i '' 's/ironsbe-fast = { path = "ironsbe-fast", version = "[^"]*"/ironsbe-fast = { path = "ironsbe-fast", version = "$(VERSION)"/' Cargo.toml
	@sed -i '' 's/ironsbe-codegen = { path = "ironsbe-codegen", version = "[^"]*"/ironsbe-codegen = { path = "ironsbe-codegen", version = "$(VERSION)"/' Cargo.toml
	@sed -i '' 's/ironsbe-derive = { path = "ironsbe-derive", version = "[^"]*"/ironsbe-derive = { path = "ironsbe-derive", version = "$(VERSION)"/' Cargo.toml
	@sed -i '' 's/ironsbe-engine = { path = "ironsbe-engine", version = "[^"]*"/ironsbe-engine = { path = "ironsbe-engine", version = "$(VERSION)"/' Cargo.toml
	@echo "Version updated to $(VERSION)"
	@cargo check --workspace

# Default target
.PHONY: all
all: test fmt lint build

# Build the project
.PHONY: build
build:
	cargo build

.PHONY: release
release:
	cargo build --release

# Run tests
.PHONY: test
test:
	LOGLEVEL=WARN cargo test

# Format the code
.PHONY: fmt
fmt:
	cargo +stable fmt --all

# Check formatting
.PHONY: fmt-check
fmt-check:
	cargo +stable fmt --check

# Run Clippy for linting
.PHONY: lint
lint:
	cargo clippy --all-targets --all-features -- -D warnings

.PHONY: lint-fix
lint-fix:
	cargo clippy --fix --all-targets --all-features --allow-dirty --allow-staged -- -D warnings

# Clean the project
.PHONY: clean
clean:
	cargo clean

# Pre-push checks
.PHONY: check
check: test fmt-check lint

# Run the project
.PHONY: run
run:
	cargo run

.PHONY: fix
fix:
	cargo fix --allow-staged --allow-dirty

.PHONY: pre-push
pre-push: fix fmt lint-fix test readme doc

.PHONY: doc
doc:
	cargo clippy -- -W missing-docs

.PHONY: doc-open
doc-open:
	cargo doc --open

.PHONY: publish
publish: readme
	@echo "Publishing to crates.io requires publishing crates in dependency order."
	@echo "Use 'make publish-all' to publish all crates in the correct order."
	@echo "Or publish individual crates with 'make publish-crate CRATE=ironsbe-core'"

.PHONY: publish-crate
publish-crate:
	@if [ -z "$(CRATE)" ]; then echo "Usage: make publish-crate CRATE=<crate-name>"; exit 1; fi
	find . -name ".DS_Store" -type f -delete | true
	cargo login ${CARGO_REGISTRY_TOKEN}
	cargo package -p $(CRATE)
	cargo publish -p $(CRATE)

.PHONY: publish-all
publish-all: readme
	@echo "Publishing all crates in dependency order..."
	find . -name ".DS_Store" -type f -delete | true
	cargo login ${CARGO_REGISTRY_TOKEN}
	@echo "1/11: Publishing ironsbe-core..."
	cargo publish -p ironsbe-core || true
	@sleep 30
	@echo "2/11: Publishing ironsbe-derive..."
	cargo publish -p ironsbe-derive || true
	@sleep 30
	@echo "3/11: Publishing ironsbe-dictionary..."
	cargo publish -p ironsbe-dictionary || true
	@sleep 30
	@echo "4/11: Publishing ironsbe-tagvalue..."
	cargo publish -p ironsbe-tagvalue || true
	@sleep 30
	@echo "5/11: Publishing ironsbe-store..."
	cargo publish -p ironsbe-store || true
	@sleep 30
	@echo "6/11: Publishing ironsbe-session..."
	cargo publish -p ironsbe-session || true
	@sleep 30
	@echo "7/11: Publishing ironsbe-transport..."
	cargo publish -p ironsbe-transport || true
	@sleep 30
	@echo "8/11: Publishing ironsbe-fast..."
	cargo publish -p ironsbe-fast || true
	@sleep 30
	@echo "9/11: Publishing ironsbe-codegen..."
	cargo publish -p ironsbe-codegen || true
	@sleep 30
	@echo "10/11: Publishing ironsbe-engine..."
	cargo publish -p ironsbe-engine || true
	@sleep 30
	@echo "11/11: Publishing ironsbe..."
	cargo publish -p ironsbe-example || true
	@echo "Done! All crates published."

.PHONY: coverage
coverage:
	export LOGLEVEL=WARN
	cargo install cargo-tarpaulin
	mkdir -p coverage
	cargo tarpaulin --exclude-files 'benches/**' --all-features --workspace --timeout 120 --out Xml

.PHONY: coverage-html
coverage-html:
	export LOGLEVEL=WARN
	cargo install cargo-tarpaulin
	mkdir -p coverage
	cargo tarpaulin --exclude-files 'benches/**' --verbose --all-features --workspace --timeout 120 --out Html --output-dir coverage

.PHONY: coverage-json
coverage-json:
	export LOGLEVEL=WARN
	cargo install cargo-tarpaulin
	mkdir -p coverage
	cargo tarpaulin --exclude-files 'benches/**' --verbose --all-features --workspace --timeout 120 --out Json --output-dir coverage

.PHONY: open-coverage
open-coverage:
	open coverage/tarpaulin-report.html

# Rule to show git log
git-log:
	@if [ "$(CURRENT_BRANCH)" = "HEAD" ]; then \
		echo "You are in a detached HEAD state. Please check out a branch."; \
		exit 1; \
	fi; \
	echo "Showing git log for branch $(CURRENT_BRANCH) against main:"; \
	git log main..$(CURRENT_BRANCH) --pretty=full

.PHONY: create-doc
create-doc:
	cargo doc --no-deps --document-private-items

.PHONY: readme
readme: create-doc
	@echo "README.md already exists (workspace project, cargo-readme not applicable)"

.PHONY: check-cargo-readme
check-cargo-readme:
	@command -v cargo-readme > /dev/null || (echo "Installing cargo-readme..."; cargo install cargo-readme)

.PHONY: check-spanish
check-spanish:
	cd scripts && python3 spanish.py ../src && cd ..

.PHONY: zip
zip:
	@echo "Creating $(ZIP_NAME) without any 'target' directories, 'Cargo.lock', and hidden files..."
	@find . -type f \
		! -path "*/target/*" \
		! -path "./.*" \
		! -name "Cargo.lock" \
		! -name ".*" \
		| zip -@ $(ZIP_NAME)
	@echo "$(ZIP_NAME) created successfully."


.PHONY: check-cargo-criterion
check-cargo-criterion:
	@command -v cargo-criterion > /dev/null || (echo "Installing cargo-criterion..."; cargo install cargo-criterion)

.PHONY: bench
bench: check-cargo-criterion
	cargo criterion --output-format=quiet

.PHONY: bench-show
bench-show:
	open target/criterion/reports/index.html

.PHONY: bench-save
bench-save: check-cargo-criterion
	cargo criterion --output-format quiet --history-id v0.4.8 --history-description "Version 0.3.2 baseline"

.PHONY: bench-compare
bench-compare: check-cargo-criterion
	cargo criterion --output-format verbose

.PHONY: bench-json
bench-json: check-cargo-criterion
	cargo criterion --message-format json

.PHONY: bench-clean
bench-clean:
	rm -rf target/criterion


.PHONY: workflow-coverage
workflow-coverage:
	DOCKER_HOST="$${DOCKER_HOST}" act push --job code_coverage_report \
       -P ubuntu-latest=catthehacker/ubuntu:latest \
       --privileged

.PHONY: workflow-build
workflow-build:
	DOCKER_HOST="$${DOCKER_HOST}" act push --job build \
       -P ubuntu-latest=catthehacker/ubuntu:latest

.PHONY: workflow-lint
workflow-lint:
	DOCKER_HOST="$${DOCKER_HOST}" act push --job lint

.PHONY: workflow-test
workflow-test:
	DOCKER_HOST="$${DOCKER_HOST}" act push --job run_tests

.PHONY: workflow
workflow: workflow-build workflow-lint workflow-test workflow-coverage

.PHONY: tree
tree:
	tree -I 'target|.idea|.run|.DS_Store|Cargo.lock|*.md|*.toml|*.zip|*.html|*.xml|*.json|*.txt|*.sh|*.yml|*.yaml|*.gitignore|*.gitattributes|*.gitmodules|*.git|*.gitkeep|*.gitlab-ci.yml' -a -L 3
