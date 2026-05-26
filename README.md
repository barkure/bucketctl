# bucketctl

Small interactive S3 client with an SFTP-like workflow.

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

## Commands

Root:
- `ls`
- `attach <profile>`
- `help`
- `exit`
- `!<local command>`
- `Ctrl-D`

Inside a bucket:
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
- `Ctrl-D` detaches back to root
