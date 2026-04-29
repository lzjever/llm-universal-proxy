# Documentation

This is the main docs entrypoint for `llmup`.

Start here based on what you need:

- [configuration.md](./configuration.md)
  Static YAML configuration, static `data_auth`, provider credential sources, provider-neutral preset sources, full field reference, and upstream proxy support
- [clients.md](./clients.md)
  Codex, Claude Code, and Gemini wrapper setup plus common client notes
- [container.md](./container.md)
  GHCR image usage, Docker Compose example, container smoke, and release policy
- [admin-dynamic-config.md](./admin-dynamic-config.md)
  Admin API, live namespace config updates, `/admin/data-auth`, CAS / revision behavior, and redacted state
- [docs/ga-readiness-review.md](./ga-readiness-review.md)
  GA scope, required release evidence, and compatibility boundaries
- [../examples/quickstart-provider-neutral.yaml](../examples/quickstart-provider-neutral.yaml)
  Provider-neutral config source for the CLI-wrapper preset path
- [../examples/quickstart-openai-minimax.yaml](../examples/quickstart-openai-minimax.yaml)
  Historical concrete OpenAI + MiniMax example; MiniMax is not the GA-required provider path
- [protocol-compatibility-matrix.md](./protocol-compatibility-matrix.md)
  Compatibility boundaries and portability summary
- [max-compat-design.md](./max-compat-design.md)
  Maximum-compatibility design, visible tool identity contract, and current multimodal boundaries
- [DESIGN.md](./DESIGN.md)
  Current architecture map for the running system

Related docs:

- [PRD.md](./PRD.md)
  Product requirements and scope
- [CONSTITUTION.md](./CONSTITUTION.md)
  Project-level invariants and non-negotiable behavior
- [protocol-baselines/README.md](./protocol-baselines/README.md)
  Protocol baseline captures and provider-specific reference material
