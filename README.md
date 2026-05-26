[English](README.md) | [简体中文](README.zh-Hans.md)

# bucketctl

A simple S3 command-line tool with an SFTP-like workflow.

## Config

Config path:

```text
~/.config/bucketctl/config.toml
```
You can define multiple profiles. Each profile maps to one bucket.

Example:

```toml
[mybucket]
bucket = "abcde"
endpoint = "https://s3.example.com"
region = "cn-east-1"
access_key = "env:ACCESS_KEY"
# access_key = "YOUR_ACCESS_KEY"
secret_key = "env:SECRET_KEY"
# secret_key = "YOUR_SECRET_KEY"
path_style = false
```

## Installation

Install the latest matching release:

```bash
bash -c "$(curl -fsSL https://github.com/barkure/bucketctl/raw/main/install.sh)" @ install
```

Remove it:

```bash
bash -c "$(curl -fsSL https://github.com/barkure/bucketctl/raw/main/install.sh)" @ remove
```

## Usage

### Command mode

```bash
bucketctl ls
bucketctl ls <mybucket>
bucketctl ls <mybucket>:/path
bucketctl put <local> <mybucket>:/path
bucketctl get <mybucket>:/path [local]
bucketctl mkdir <mybucket>:/path
bucketctl rm <mybucket>:/path
bucketctl rm -r <mybucket>:/path
```

### Interactive mode

```bash
bucketctl <mybucket>
```

Available commands:
- `ls [path]`
- `cd [path]`
- `pwd`
- `mkdir <remote-dir>`
- `put <local> [remote]`
- `get <remote> [local]`
- `rm <remote>`
- `rm -r <remote-dir>`
- `help`
- `exit`
- `!<local command>`
- `Ctrl-C` cancels the current transfer
- `Ctrl-D` exits the shell

### Alias (optional)

Add to your shell config file:

```bash
# For bash (~/.bashrc)
echo "alias bkt='bucketctl'" >> ~/.bashrc

# For zsh (~/.zshrc)
echo "alias bkt='bucketctl'" >> ~/.zshrc

# For fish (~/.config/fish/config.fish)
echo "alias bkt='bucketctl'" >> ~/.config/fish/config.fish
```

Reload your shell, then use the `bkt` command instead of `bucketctl`.
