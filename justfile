default:
    @just --list

# Install the git hooks (pre-push: ticket IDs, then fmt, clippy and tests)
hooks:
    git config core.hooksPath .githooks

# What CI runs
check:
    cargo fmt -- --check
    cargo clippy --all-targets --all-features -- -D warnings
    cargo test --all-features
    cargo deny check

# The tests against the real offline binaries: just test-real ~/unforged/pack nib.lic ~/unforged/kit/files
test-real pack licence files:
    UNFORGED_TEST_BINARY={{pack}}/unforged-nib UNFORGED_TEST_LOUPE_BINARY={{pack}}/unforged-loupe \
    UNFORGED_TEST_LICENCE={{licence}} UNFORGED_TEST_FILES={{files}} cargo test --test real_binaries -- --nocapture
