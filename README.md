[English](README.md) | [简体中文](README.zh-Hans.md)

# bucketctl

A simple S3 command-line tool with an SFTP-like workflow.

<img src="assets/demo.gif" alt="Demo" width="600" />

## Config

Config path:

```text
~/.config/bucketctl/config.toml
```

Override it with `-c <PATH>` or `--config <PATH>`:

```bash
bucketctl -c ./my-config.toml
```

Define multiple profiles — one per bucket:

```toml
[settings]
default_profile = "bitiful"

[bitiful]
bucket = "abcde"
endpoint = "https://s3.bitiful.net"
region = "cn-east-1"
access_key = "env:ACCESS_KEY"
# access_key = "YOUR_ACCESS_KEY"
secret_key = "env:SECRET_KEY"
# secret_key = "YOUR_SECRET_KEY"
path_style = false

[cloudflare-r2]
bucket = "assets"
endpoint = "https://xxx.r2.cloudflarestorage.com"
region = "auto"
access_key = "xxx"
secret_key = "xxx"
path_style = true
```

`default_profile` is optional. When set, most commands target that bucket by default.

## Installation

Install or update the latest matching release:

```bash
bash -c "$(curl -fsSL https://github.com/barkure/bucketctl/raw/main/install.sh)" @ install
```

Remove it:

```bash
bash -c "$(curl -fsSL https://github.com/barkure/bucketctl/raw/main/install.sh)" @ remove
```

## Usage

### List buckets

```bash
$ bucketctl
bitiful  cloudflare-r2
```

### Command Mode

Operate on the **default bucket** directly:

```bash
bucketctl ls /                  # list root of the default bucket
bucketctl ls /path/to/dir       # list a subdirectory
bucketctl put ~/a.txt /path     # upload
bucketctl get /file ./          # download
bucketctl mkdir /new-dir        # create directory
bucketctl rm /file              # delete file
bucketctl rm -r /dir            # delete directory recursively
```

Target a **specific bucket** with `<profile>:` prefix or just the profile name (for `ls`):

```bash
bucketctl ls cloudflare-r2              # list that bucket's root
bucketctl ls cloudflare-r2:/2023        # list a subdirectory
bucketctl put ./a.txt cloudflare-r2:/   # upload
bucketctl get cloudflare-r2:/file ./    # download
```

| Command | Default bucket | Specific bucket |
|---------|---------------|-----------------|
| List objects | `bucketctl ls /path` | `bucketctl ls <profile>:/path` |
| Upload | `bucketctl put ./a.txt /path` | `bucketctl put ./a.txt <profile>:/path` |
| Download | `bucketctl get /file ./` | `bucketctl get <profile>:/file ./` |
| Create dir | `bucketctl mkdir /dir` | `bucketctl mkdir <profile>:/dir` |
| Delete file | `bucketctl rm /file` | `bucketctl rm <profile>:/file` |
| Delete dir | `bucketctl rm -r /dir` | `bucketctl rm -r <profile>:/dir` |

> `~` in local paths is expanded to your home directory.

### Interactive Mode

Enter the REPL for a bucket:

```bash
bucketctl <profile>
```

Available commands:

| Command | Description |
|---------|-------------|
| `ls [path]` | List directory |
| `cd [path]` | Change directory |
| `pwd` | Print working directory |
| `mkdir <remote-dir>` | Create directory |
| `put <local> [remote]` | Upload file |
| `get <remote> [local]` | Download file |
| `rm <remote>` | Delete file |
| `rm -r <remote-dir>` | Delete directory recursively |
| `help` | Show help |
| `exit` / `Ctrl-D` | Exit REPL |
| `!<cmd>` | Run local shell command |
| `Ctrl-C` | Cancel current transfer |

### Alias (Optional)

Add to your shell config file:

```bash
# bash (~/.bashrc) or zsh (~/.zshrc)
alias bkt='bucketctl'

# fish (~/.config/fish/config.fish)
alias bkt='bucketctl'
```

Reload your shell, then use `bkt` instead of `bucketctl`.
