# Templates

Kurogane provides built-in templates to help you start new applications.

## Available templates

### Vanilla

A minimal application with no frontend framework.

Best for:
- Learning the runtime
- Building from scratch

### SPA

A frontend-driven template designed for frameworks like React, Vue or Vite.

Supports:
- External development servers
- Modern frontend tooling

Best for:
- Rapid iteration workflows

### IPC

Demonstrates communication between the frontend and backend.

Includes:
- Command registration
- JSON-based messaging
- Structured responses

Best for:
- Apps with backend logic

## Usage

Create a new project with a template:

```bash
kurogane init --template <name>
```

Example:

```bash
kurogane init --template spa
```
