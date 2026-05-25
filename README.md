# bucketctl

Small interactive S3 client with an SFTP-like workflow.

One profile maps to one bucket.

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

## Run

```bash
cargo run --
```

Directly attach a profile:

```bash
cargo run -- mybucket
```

## Flow

```text
bucketctl > ls
mybucket

bucketctl > attach mybucket
mybucket:/ >
```

Inside the bucket:

```text
mybucket:/ > ls
mybucket:/ > cd some/path
mybucket:/ > put ./local.txt
mybucket:/ > get remote.bin
mybucket:/ > rm remote.bin
mybucket:/ > rm -r some/prefix
```

## Commands

At `bucketctl >`:

- `ls`
- `attach <profile>`
- `help`
- `exit`
- `!<local command>`
- `Ctrl-D`

At `mybucket:/ >`:

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
- `Ctrl-D` detaches back to `bucketctl >`
