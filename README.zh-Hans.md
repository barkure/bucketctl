[English](README.md) | [简体中文](README.zh-Hans.md)

# bucketctl

一个简单的 S3 命令行工具，使用方式接近 SFTP。

<img src="assets/demo.gif" alt="演示" width="600" />

## 配置

配置文件路径：

```text
~/.config/bucketctl/config.toml
```

也可以通过 `-c <PATH>` 或 `--config <PATH>` 指定其他路径：

```bash
bucketctl -c ./my-config.toml
```

可以定义多个 profile，每个 profile 对应一个桶（bucket）：

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

`default_profile` 是可选的。设置后，大部分命令默认操作该桶。

## 安装

安装或更新最新 release：

```bash
bash -c "$(curl -fsSL https://github.com/barkure/bucketctl/raw/main/install.sh)" @ install
```

卸载：

```bash
bash -c "$(curl -fsSL https://github.com/barkure/bucketctl/raw/main/install.sh)" @ remove
```

## 使用

### 列出桶

```bash
$ bucketctl
bitiful  cloudflare-r2
```

`bucketctl ls` 列出默认桶内容：

```bash
$ bucketctl ls
       DIR                test/
      22 B  May 27 17:10  hello.txt
  48.9 MiB  May 27 16:56  vlc-3.0.23-arm64.dmg
```

### 命令模式

直接操作**默认桶**：

```bash
bucketctl ls                      # 列出默认桶根目录
bucketctl ls /path/to/dir         # 列出子目录
bucketctl put ~/a.txt /path     # 上传
bucketctl get /file ./          # 下载
bucketctl mkdir /new-dir        # 创建目录
bucketctl rm /file              # 删除文件
bucketctl rm -r /dir            # 递归删除目录
```

操作**指定桶**，加上 `<profile>:` 前缀或直写 profile 名（`ls` 支持）：

```bash
bucketctl ls cloudflare-r2              # 列出该桶根目录
bucketctl ls cloudflare-r2:/2023        # 列出子目录
bucketctl put ./a.txt cloudflare-r2:/   # 上传
bucketctl get cloudflare-r2:/file ./    # 下载
```

| 命令 | 默认桶 | 指定桶 |
|------|-------|-------|
| 列出根目录 | `bucketctl ls` | `bucketctl ls <profile>` |
| 列出路径 | `bucketctl ls /path` | `bucketctl ls <profile>:/path` |
| 上传 | `bucketctl put ./a.txt /path` | `bucketctl put ./a.txt <profile>:/path` |
| 下载 | `bucketctl get /file ./` | `bucketctl get <profile>:/file ./` |
| 创建目录 | `bucketctl mkdir /dir` | `bucketctl mkdir <profile>:/dir` |
| 删除文件 | `bucketctl rm /file` | `bucketctl rm <profile>:/file` |
| 删除目录 | `bucketctl rm -r /dir` | `bucketctl rm -r <profile>:/dir` |

> 本地路径中的 `~` 会被展开为用户主目录。

### 交互模式

进入指定桶的 REPL：

```bash
bucketctl <profile>
```

可用命令：

| 命令 | 说明 |
|------|------|
| `ls [path]` | 列出目录 |
| `cd [path]` | 切换目录 |
| `pwd` | 显示当前路径 |
| `mkdir <remote-dir>` | 创建目录 |
| `put <local> [remote]` | 上传文件 |
| `get <remote> [local]` | 下载文件 |
| `rm <remote>` | 删除文件 |
| `rm -r <remote-dir>` | 递归删除目录 |
| `help` | 显示帮助 |
| `exit` / `Ctrl-D` | 退出 REPL |
| `!<cmd>` | 执行本地命令 |
| `Ctrl-C` | 取消当前传输 |

### 别名（可选）

追加到 shell 配置文件：

```bash
# bash (~/.bashrc) 或 zsh (~/.zshrc)
alias bkt='bucketctl'

# fish (~/.config/fish/config.fish)
alias bkt='bucketctl'
```

重载 shell 后，就可以用 `bkt` 代替 `bucketctl`。
