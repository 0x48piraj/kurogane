# Templates

New Kurogane applications are generated from templates (`kurogane new`). Existing projects get a Rust shell added instead (`kurogane init`).

Every Kurogane template is a [cargo-generate](https://github.com/cargo-generate/cargo-generate) template. The official starters are too. Kurogane adds its own starter selection on top. A template you write works with `kurogane new` and with plain `cargo generate`.

## Official starters

Kurogane has official starters for common frontend setups. Each one comes in TypeScript and JavaScript.

The starters and their source code live in the [`kurogane-rs`](https://github.com/orgs/kurogane-rs/) repositories:

| Starter | Repository | Description |
|---------|------------|-------------|
| `minimal` | [`kurogane-rs/kurogane-starter-minimal`](https://github.com/kurogane-rs/kurogane-starter-minimal) | Bare-bones project with a Vite frontend |
| `react` | [`kurogane-rs/kurogane-starter-react`](https://github.com/kurogane-rs/kurogane-starter-react) | React + Vite |
| `svelte` | [`kurogane-rs/kurogane-starter-svelte`](https://github.com/kurogane-rs/kurogane-starter-svelte) | Svelte 5 + Vite |
| `vue` | [`kurogane-rs/kurogane-starter-vue`](https://github.com/kurogane-rs/kurogane-starter-vue) | Vue 3 + Vite |

Without arguments `kurogane new` asks for a starter and a language. You can also name the starter:

```bash
kurogane new react
```

```bash
kurogane new react --language javascript --name my-app
```

The CLI fetches each starter from its repository's default branch.

## Custom templates

Use `--template` to generate from any other template. A starter name and `--template` cannot be used together.

```bash
kurogane new --template gh:user/repo
```

```bash
kurogane new --template https://gitlab.com/user/repo
```

```bash
kurogane new --template ./my-template
```

`--template` takes a local path, a git URL or a cargo-generate shorthand (`gh:`, `gl:`, `bb:`, `sr:` or `user/repo` for GitHub). Private repositories use your git credentials and SSH agent.

A git template comes from its default branch. Every run fetches it again into a snapshot in Kurogane's cache. A run that cannot fetch it generates from the last snapshot and says how old it is. `kurogane clean` removes the snapshots.

### Placeholder values

A template's placeholders are asked for interactively. `--values` takes them from a TOML file instead:

```toml
[values]
language = "javascript"
```

```bash
kurogane new --template gh:user/repo --name my-app --values answers.toml --ci
```

A values file gives the same answers on every run and every machine. `--language` sets the `language` placeholder for any template.

The project directory takes the project's name in kebab-case.

Generated projects are not initialized as git repositories and are never added to a parent Cargo workspace.

## Adding Kurogane to an existing app

`kurogane init` adds Kurogane to an existing frontend project. It generates the Rust shell in the current directory. The shell is `Cargo.toml`, `build.rs`, `kurogane.toml` and `src/main.rs`.

_It never overwrites existing files or an existing Kurogane setup._ Its guardrails:

* Refuses to run when `kurogane.toml` already exists.
* Refuses to run when the directory already has a `Cargo.toml` or `src/main.rs`.
* Asks for the build output directory and the dev server URL unless `--assets` and `--dev-url` give them. A non-interactive run needs both flags.

`--assets` becomes `frontend-dist` in `kurogane.toml`. `kurogane bundle` packages that directory.

Debug builds load the dev server URL. The prompt always asks for one. Pass `--dev-url ""` to make debug builds load the build output instead. Release builds load the bundle's `content/`.

## Hooks and safety

A template can run Rhai hooks (`[hooks]` in its `cargo-generate.toml`) during generation. cargo-generate asks before a hook runs any command and shows the command.

`--yes` lets hooks run commands without asking. A non-interactive run without `--yes` refuses any command a hook asks for.

> [!CAUTION]
> Only pass `--yes` for templates you trust. Hook scripts are never inspected or analyzed.

The official starters have no hooks.

## Post-generation setup

Generated projects need no setup of their own. `kurogane dev` installs the Chromium runtime the project uses when it is missing. The application loads it when it starts. A template needs no `.cargo/config.toml` or linker flags for it. See [bundling](bundling.md) for how a bundle carries its runtime.

## Authoring templates

A Kurogane template is a cargo-generate template that produces a Kurogane application. Start from an official starter. Fork it, change it and publish it.

### Layout

```
Cargo.toml           # the application crate, depending on kurogane and kurogane-build
build.rs             # calls kurogane_build::build()
kurogane.toml        # the Kurogane application manifest
src/main.rs          # the entry point
frontend/            # a self-contained frontend project
cargo-generate.toml  # placeholders and conditional files
```

`cargo-generate.toml` is read during generation and never copied into the project.

### The build script

Every Kurogane application's `build.rs` calls `kurogane_build::build()`:

```rust
fn main() {
    kurogane_build::build();
}
```

`kurogane-build` is one of its build dependencies:

```toml
[build-dependencies]
kurogane-build = { git = "https://github.com/0x48piraj/kurogane" }
```

On Windows it embeds the application manifest CEF's own executables carry. Without it Windows tells the application and the Chromium inside it that they run on Windows 8. `kurogane bundle` also writes the manifest beside a Windows bundle's executable for an application built without it.

### Placeholders

Every file is rendered with [Liquid](https://shopify.github.io/liquid/). cargo-generate always provides `{{project-name}}` (as typed) and `{{crate_name}}` (snake case):

```toml
[package]
name = "{{crate_name}}"
```

Declare your own placeholders in `cargo-generate.toml`. A placeholder with a default and choices takes its value from `--language`, `--values`, the prompt or its default under `--ci`:

```toml
[placeholders.language]
type = "string"
prompt = "Language"
default = "typescript"
choices = ["typescript", "javascript"]
```

Keep placeholders for real choices. Values every project shares belong in the files as they are.

### Language variants

The official starters keep both languages in one `frontend/` folder. Files only one language needs are left out by condition:

```toml
[conditional.'language == "typescript"']
ignore = ["frontend/src/main.js"]

[conditional.'language == "javascript"']
ignore = ["frontend/src/main.ts", "frontend/tsconfig.json"]
```

Files both languages share differ inline:

```html
<script type="module" src="/src/main.{% if language == "typescript" %}ts{% else %}js{% endif %}"></script>
```

This needs no hook. Generating the starter asks for no consent.

### Frontend syntax that looks like Liquid

Wrap template syntax of your frontend framework in `{% raw %}` so Liquid leaves it alone:

```vue
<h1>{% raw %}{{ message }}{% endraw %}</h1>
```

### Testing

Generate from your working copy once per variant:

```bash
kurogane new --template ./my-template --name check-ts --language typescript --ci
```

```bash
kurogane new --template ./my-template --name check-js --language javascript --ci
```

Then install, build and run each project as a user would.

### Publishing

Push the template to a git host. Users generate from it by its shorthand:

```bash
kurogane new --template gh:you/kurogane-template
```

Pin Kurogane by tag so generated projects build the same way every time. Pin `kurogane-build` to the same tag:

```toml
[dependencies]
kurogane = { git = "https://github.com/0x48piraj/kurogane", tag = "v0.0.6-alpha.2" }

[build-dependencies]
kurogane-build = { git = "https://github.com/0x48piraj/kurogane", tag = "v0.0.6-alpha.2" }
```

### Kurogane application manifest

```toml
[app]
name = "{{project-name}}"
frontend = "frontend"                                # source npm project root
frontend-dist = "frontend/dist"                      # build output; what gets packaged
frontend-install = "npm --prefix frontend install"   # installs the frontend's dependencies
frontend-run = "npm --prefix frontend run dev"       # starts the dev server
frontend-build = "npm --prefix frontend run build"   # run by kurogane bundle before cargo build
```

`frontend` names the source npm project (`frontend/`). `frontend-dist` names the compiled output `kurogane bundle` packages (`frontend/dist`). Both are build-time paths relative to the project root.

`kurogane new` prints `frontend-install` and `frontend-run` in its **Next steps**. They install the frontend's dependencies and start its dev server before `kurogane dev`.

`frontend-build` is the command `kurogane bundle` runs to build the frontend before packaging. It runs from the workspace root through the platform shell. That is `sh` on Linux and macOS and `cmd` on Windows. Shell syntax such as `&&` works.

In release builds a generated `src/main.rs` loads the bundle's `content/` directory instead of `frontend-dist`:

```rust
#[cfg(not(debug_assertions))]
App::new("content").run_or_exit();
```

`frontend-dist` is a build-time path only; the packaged app always serves its frontend from `content/` inside the bundle.

### Directory convention

Official starters keep the frontend as a self-contained Vite project in `frontend/`. The layout has three layers:

```
frontend/      # self-contained frontend source
frontend/dist/ # build output (Vite default outDir)
content/       # bundle-internal (copied by bundler at package time)
```

`frontend/` is a normal Vite project. `cd frontend && npm install && npm run dev` works as in any Vite app. During development the Vite dev server serves `frontend/` and the desktop app loads it from the dev URL.

`npm --prefix frontend run build` compiles the frontend into `frontend/dist`. `kurogane bundle` copies that output into `content/`. The packaged app serves its frontend from there.
