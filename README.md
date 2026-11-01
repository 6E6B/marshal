<div align="center">

<img src="data/io.github._6E6B.marshal.svg" alt="" width="96" height="96">

# Marshal

Run your MCP servers once and share them with every AI agent on your computer.

</div>

Normally every coding agent you use (Claude Code, Codex, Cursor, Zed…) starts its own copy of each MCP server. Marshal runs each server once and lets all your agents connect to it, so you have one place to start and stop servers, try their tools, read their logs and see which agent is calling what.

![Server overview](docs/screenshots/overview.png)

## What you can do

**Manage your servers.** Add local (command-line) or remote (HTTP) MCP servers, start and stop them, and group them into profiles you can launch together. Browse the MCP registry to find new ones. Secrets like API keys are kept in your system keyring.

**Try tools directly.** See every tool a server offers and run it with your own inputs, without going through an agent.

![Tool runner](docs/screenshots/tool-runner.png)

**See what your agents are doing.** The Traffic view shows each request, which agent sent it, and how long it took. Server logs are kept in the app too.

![Traffic per client](docs/screenshots/traffic.png)

**Connect your agents in one click.** Marshal finds the agents installed on your computer and adds the server to their settings for you, without touching anything else in the file. It can also go the other way and import servers you've already set up in an agent.

Supported: Claude Code, OpenAI Codex, Cursor, VS Code, Zed, Windsurf, GitHub Copilot CLI, Google Antigravity, OpenCode, Amp, Devin, Kiro, Cline, Roo Code, Continue, Goose and Pi.

**Use your servers from anywhere.** Share a server over the internet through Cloudflare, ngrok, Tailscale Funnel or OpenAI's tunnel, and copy the connection URL with one click. Handy for using your local tools from a web-based agent or another machine.

![Remote access via Cloudflare](docs/screenshots/remote-access.png)

**Stay out of the way.** Marshal can live in the system tray, start at login and keep servers running in the background. There's also a small command line for scripts:

```sh
marshal list              # show servers and whether they're running
marshal start <server>    # also: stop, restart
marshal profile <name>    # start a group of servers
```

## Install

Build and run from source:

```sh
cargo run --release
```

Or build the Flatpak:

```sh
flatpak-builder --user --install --force-clean builddir flatpak/io.github._6E6B.marshal.yml
flatpak run io.github._6E6B.marshal
```

To share servers remotely, install the tunnel tool you want to use (`cloudflared`, `ngrok`, `tailscale`…) on your system.

## License

GPL-3.0-or-later. See [LICENSE](LICENSE).
