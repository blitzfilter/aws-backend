# Blitzfilter AWS Backend

Blitzfilter AWS Backend is a Rust-based serverless application that provides web scraping and item management services for AWS. The system consists of multiple Lambda functions, API Gateway handlers, and supporting services that work with DynamoDB, OpenSearch, and SQS.

Always reference these instructions first and fallback to search or bash commands only when you encounter unexpected information that does not match the info here.

## Working Effectively

### Bootstrap and Build
- Install Rust toolchain: `rustup install stable && rustup default stable`
- Bootstrap the workspace: `cd /home/runner/work/aws-backend/aws-backend`
- **NEVER CANCEL**: Check dependencies: `cargo check --workspace` -- takes 3 minutes on first run with downloads. Set timeout to 10+ minutes.
- **NEVER CANCEL**: Build workspace: `cargo build --workspace` -- takes 3 minutes 11 seconds. Set timeout to 10+ minutes.

### Lint and Format
- Format check: `cargo fmt --all -- --check` -- takes 0.5 seconds
- Lint check: `cargo clippy --workspace --all-targets --all-features -- -D warnings` -- takes 15 seconds

### Testing
- **Unit tests**: `cargo test --workspace --lib --all-features` -- takes 35 seconds. Set timeout to 2+ minutes. Apply parameterized testing with crate `rstest` when plausible, e.g. for serialization.
- **Integration tests**: Require additional setup (see Prerequisites section)
- All test names need to follow a consistent naming convention, e.g., `should_[expectation]_when_[condition]_for_[purpose]`. The placeholders can be replaced with many words, e.g. `should_serialize_data_when_valid_for_storing`. Provide meaningful test-names that describe the purpose of the test.
- Most types have an instance for `fake::Dummy<fake::Faker>`. Our internal crates provide this functionality via feature-flag `test-data`. You may need to include it for dev-dependencies. Use it to generate test data when plausible.

### Prerequisites for Full Integration Testing
**WARNING**: These require network access and may fail in restricted environments:
- Install Zig: `npm install -g @ziglang/cli` -- may fail due to network restrictions
- **NEVER CANCEL**: Install cargo-lambda: `cargo install cargo-lambda` -- takes 15+ minutes. Set timeout to 30+ minutes.
- Docker must be available for LocalStack containers

## Validation

### Always Validate Changes
- **ALWAYS** run format and lint checks before committing: `cargo fmt --all -- --check && cargo clippy --workspace --all-targets --all-features -- -D warnings`
- **ALWAYS** run unit tests after code changes: `cargo test --workspace --lib --all-features`
- Run integration tests when changing core functionality: `cargo test --workspace --all-features --test '*'`

### Manual Testing Scenarios
Since this is a serverless backend, manual testing involves:
1. **Build validation**: Ensure all Lambda functions compile: `cargo build --workspace`
2. **Unit test validation**: Verify business logic: `cargo test --workspace --lib --all-features`
3. **Integration test validation**: Test AWS service integration with LocalStack containers
4. **CI validation**: The CI pipeline (.github/workflows/ci.yml) runs the complete test suite

### Limitations
- **Cannot run Lambda functions locally** without cargo-lambda and proper AWS setup
- **OpenSearch tests often timeout** in CI environments (5+ minutes, often fail)
- **Full integration testing requires network access** for tool installation
- **No CLI applications** - all components are Lambda functions or libraries

## Critical Timing and Timeout Information

**NEVER CANCEL these commands - they are expected to take significant time:**
- `cargo check --workspace`: 3 minutes (first run with downloads) - Set timeout to 10+ minutes
- `cargo build --workspace`: 3 minutes 11 seconds - Set timeout to 10+ minutes
- `cargo install cargo-lambda`: 15+ minutes - Set timeout to 30+ minutes
- DynamoDB integration tests: 41 seconds - Set timeout to 2+ minutes
- OpenSearch integration tests: 5+ minutes (often timeout) - Set timeout to 10+ minutes
- Unit tests: 35 seconds - Set timeout to 2+ minutes

## Project Structure

### Key Modules
- **src/common**: Shared types, API utilities, error handling, batch processing
- **src/filter**: Item filtering logic
- **src/item**: Core item management system with multiple sub-modules:
  - `item-core`: Core business logic and domain models
  - `item-dynamodb`: Data access layer for items in DynamoDB
  - `item-opensearch`: Data access layer for items in OpenSearch
  - `item-api`: API Gateway handlers
  - `item-lambda`: Lambda function implementations
- **src/scrape**: Web scraping functionality
- **src/test-api**: Testing utilities and integration test framework

### Lambda Functions (Executables)
Located in `src/item/src/item-lambda/src/`:
- `item-lambda-write-new`: Handle new item creation events
- `item-lambda-write-update`: Handle item update events
- `item-lambda-materialize-dynamodb-new`: Materialize new items to DynamoDB
- `item-lambda-materialize-dynamodb-update`: Materialize item updates to DynamoDB
- `item-lambda-materialize-opensearch-new`: Materialize new items to OpenSearch
- `item-lambda-materialize-opensearch-update`: Materialize item updates to OpenSearch

### API Handlers
- `src/item/src/item-api/src/item-api-get-item`: API Gateway handler for retrieving items

## Common Tasks

### Building Specific Components
- Build all Lambda functions: `cargo build --workspace`
- Build specific Lambda: `cd src/item/src/item-lambda/src/item-lambda-write-new && cargo build --all-features`
- Build API handlers: `cd src/item/src/item-api/src/item-api-get-item && cargo build --all-features`

### Testing Specific Components
- Test core logic: `cd src/item/src/item-core && cargo test --lib --all-features`
- Test API layer: `cd src/item/src/item-api/src/item-api-get-item && cargo test --lib --all-features`
- **Integration tests**: `cd src/item/src/item-api/src/item-api-get-item && cargo test --test '*' --all-features` (requires cargo-lambda)

### Code Quality
- Format all code: `cargo fmt --all`
- Check formatting: `cargo fmt --all -- --check`
- Lint all code: `cargo clippy --workspace --all-targets --all-features -- -D warnings`

## Frequently Referenced Files

### Workspace Configuration
```
Cargo.toml (workspace root)
├── Dependencies and workspace member definitions
└── Shared dependency versions across all crates
```

### Main Source Directories
```
src/
├── common/          # Shared utilities, API types, error handling
├── item/           # Item management system (main business logic)
│   ├── src/item-core/      # Domain models and business rules
│   ├── src/item-read/      # Data access for reading
│   ├── src/item-write/     # Data access for writing
│   ├── src/item-opensearch/     # Search and indexing
│   ├── src/item-api/       # API Gateway handlers
│   └── src/item-lambda/    # Lambda function implementations
├── filter/         # Item filtering logic
├── scrape/         # Web scraping functionality
└── test-api/       # Testing framework and integration tests
```

### CI/CD Configuration
- `.github/workflows/ci.yml`: Complete CI pipeline with lint, build, test phases
- `sonar-project.properties`: SonarQube configuration for code quality analysis

## Troubleshooting

### Build Issues
- **"cargo command not found"**: Install Rust toolchain: `curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh`
- **Network timeouts during install**: Expected - retry with longer timeouts
- **Integration tests fail**: Ensure Docker is available and running

### Test Issues
- **Lambda tests fail**: Requires cargo-lambda installation (`cargo install cargo-lambda`)
- **OpenSearch tests timeout**: Expected in CI environments - focus on unit tests
- **DynamoDB tests slow**: Normal - uses Docker containers for LocalStack

### Performance
- **First build is slow**: Expected - downloads all dependencies (~3 minutes)
- **Subsequent builds are faster**: Rust incremental compilation works well
- **Integration tests are slow**: LocalStack container startup adds overhead
