

.PHONY: test
test:
	poetry run maturin develop -r && poetry run pytest tests/test_geohasher.py


.PHONY: format
format: ## Format the code
	$(info --- Rust format ---)
	cargo fmt
	$(info --- Python format ---)
	poetry run ruff check . --fix
	poetry run ruff format .
