# Templates

New Kurogane applications are generated from templates (`kurogane new`). Existing projects get a Rust shell added instead (`kurogane init`).

Kurogane uses [cargo-generate](https://github.com/cargo-generate/cargo-generate) under the hood, while adding its own template selection, safety checks and post-generation setup.

## Official starters

Kurogane includes built-in starters for common frontend setups, available in both TypeScript and JavaScript.

The starters and their source code are available in the [`kurogane-rs`](https://github.com/orgs/kurogane-rs/) repositories:

| Starter | Repository | Description |
|---------|------------|-------------|
| `minimal` | [`kurogane-rs/starter-minimal`](https://github.com/kurogane-rs/starter-minimal) | Bare-bones project with a Vite frontend |
| `react` | [`kurogane-rs/starter-react`](https://github.com/kurogane-rs/starter-react) | React + Vite |
| `svelte` | [`kurogane-rs/starter-svelte`](https://github.com/kurogane-rs/starter-svelte) | Svelte 5 + Vite |
| `vue` | [`kurogane-rs/starter-vue`](https://github.com/kurogane-rs/starter-vue) | Vue 3 + Vite |

When no arguments are given, `kurogane new` prompts you to choose a starter and language interactively. You can also pass the starter name directly:

```bash
kurogane new react         # prompts for TypeScript or JavaScript
kurogane new minimal --yes # skip hook confirmation
```

## Custom templates

Use `--template` to generate from a custom template instead of an official starter. A starter name and `--template` cannot be used together.

## Template caching

Templates are cached after the first generation, so updates to a remote template won't be picked up automatically.

To pick up changes to a remote template, clear the cache with `kurogane clean` and generate again.

> Using a local template skips the cache, as you'd expect.

### Usage

```bash
kurogane new --template user/repo     # any git-hosted template
kurogane new --template gh:user/repo  # alternative
kurogane new --template ./my-template # local path
```

The project directory is named after the project, converted to kebab-case.

Generated projects are not initialized as git repositories and are never added to a parent Cargo workspace.

## Adding Kurogane to an existing app

`kurogane init` adds Kurogane to an existing frontend project.

It generates the Rust shell in the current directory and takes a deliberately conservative approach: _it never overwrites existing files or an existing Kurogane setup_.

That means a few guardrails are built in:

- Refuses to run if `kurogane.toml` already exists.
- Points empty directories at `kurogane new` instead.
- Aborts if any existing Rust project is already present.
- Asks for the assets directory and dev server URL (or takes them from `--assets` / `--dev-url`).

## Hooks and safety

Templates can run Rhai hooks (`[hooks]` in its `cargo-generate.toml`) during generation. Because those hooks execute code, Kurogane asks you to confirm them before generation.

> [!CAUTION]
> Non-interactive runs must pass `--yes`. Hook scripts themselves are never inspected or analyzed.

## Post-generation setup

Generated projects include a `.cargo/config.toml` that makes the bundled Chromium runtime available at runtime without requiring environment variables. See [bundling](bundling.md) for more details.

## Authoring templates

Kurogane templates are ordinary cargo-generate templates with a Kurogane application manifest (`kurogane.toml`) and a Rust entry point.

Liquid placeholders follow the [cargo-generate template guide](https://cargo-generate.github.io/cargo-generate/templates.html).

### Kurogane application manifest

```toml
[app]
name = "{{project-name}}"
frontend = "{{frontend_dist}}"       # source-time path to built frontend
frontend-build = "npm run build"     # command run by kurogane bundle before cargo build
```

The `frontend-build` field tells `kurogane bundle` how to produce frontend assets before packaging. When present, the bundler runs this command from the
workspace root (and skips it if `package.json` is absent).

The command is executed directly rather than through a shell, so shell syntax such as pipes and `&&` is not supported.

### Directory convention

Official starters use a three-layer layout:

```
frontend/      # source (Vite root)
frontend/dist/ # build output (Vite default outDir)
content/       # bundle-internal (copied by bundler at package time)
```

The frontend is built into `frontend/dist`, then `kurogane bundle` copies that output into `content/`. The packaged application serves its frontend from `content/`.
