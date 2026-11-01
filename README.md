# Marshal

Marshal is an MCP server manager for GNOME.

![Server overview](docs/screenshots/overview.png)

## Functions

- Add local (command) servers and remote (HTTP) servers.
- Start and stop servers.
- Start a group of servers with a profile.
- Find servers in the MCP registry.
- Keep secrets, for example API keys, in the system keyring.
- Show the tools of a server. Use a tool with your own inputs.
- Show each request, the agent that sent it, and its duration.
- Show the server logs.
- Add a server to the configuration file of an agent. Marshal does not change other entries in the file.
- Import servers from the configuration file of an agent.
- Share a server on the internet through Cloudflare, ngrok, Tailscale Funnel or the OpenAI tunnel.
- Keep servers in operation from the system tray.
- Start Marshal when you log in.

## Supported agents

Claude Code, OpenAI Codex, Cursor, VS Code, Zed, Windsurf, GitHub Copilot CLI, Google Antigravity, OpenCode, Amp, Devin, Kiro, Cline, Roo Code, Continue, Goose and Pi.

## Command line

```sh
marshal list              # Show the servers and their status
marshal start <server>    # Start a server
marshal stop <server>     # Stop a server
marshal restart <server>  # Stop and start a server
marshal profile <name>    # Start the servers in a profile
```

## Install

### From source

```sh
cargo run --release
```

### Flatpak

```sh
flatpak-builder --user --install --force-clean builddir flatpak/io.github._6E6B.marshal.yml
flatpak run io.github._6E6B.marshal
```

### Remote access

To share a server on the internet, install the applicable tunnel tool. Examples: `cloudflared`, `ngrok`, `tailscale`.

## License

GPL-3.0-or-later. Refer to [LICENSE](LICENSE).
