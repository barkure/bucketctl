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
bucketctl -c ./my-config.toml ls
bucketctl --config ~/work/bucketctl.toml mybucket
```

可以定义多个 profile。每个 profile 对应一个固定桶（bucket）。

示例：

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

## 安装

安装最新匹配当前系统的 release：

```bash
bash -c "$(curl -fsSL https://github.com/barkure/bucketctl/raw/main/install.sh)" @ install
```

卸载：

```bash
bash -c "$(curl -fsSL https://github.com/barkure/bucketctl/raw/main/install.sh)" @ remove
```

## 使用

### 命令模式：

```bash
bucketctl ls
bucketctl ls <mybucket>:/path
bucketctl get <mybucket>:/path/file .
```

### 交互模式

```bash
bucketctl <mybucket>
```

进入后可用：
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
- `Ctrl-C` 取消当前传输
- `Ctrl-D` 退出 shell

### Alias (可选)

把下面这行追加到你的 shell 配置文件：

```bash
# For bash (~/.bashrc)
echo "alias bkt='bucketctl'" >> ~/.bashrc

# For zsh (~/.zshrc)
echo "alias bkt='bucketctl'" >> ~/.zshrc

# For fish (~/.config/fish/config.fish)
echo "alias bkt='bucketctl'" >> ~/.config/fish/config.fish
```

重载 shell 后，就可以用 `bkt` 命令代替 `bucketctl`。
