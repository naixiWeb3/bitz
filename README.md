# BITZ Collector

A command line interface for BITZ cryptocurrency collecting.

## 📦 Install

To install the CLI, use [cargo](https://doc.rust-lang.org/cargo/getting-started/installation.html):

```sh
cargo install bitz
```


### Dependencies
If you run into issues during installation, please install the following dependencies for your operating system and try again:

#### Linux
```
sudo apt-get install openssl pkg-config libssl-dev
```

#### MacOS
```
brew install openssl pkg-config

# If you encounter issues with OpenSSL, you might need to set the following environment variables:
export PATH="/usr/local/opt/openssl/bin:$PATH"
export LDFLAGS="-L/usr/local/opt/openssl/lib"
export CPPFLAGS="-I/usr/local/opt/openssl/include"
```

#### Windows
```
choco install openssl pkgconfiglite
```

## ⛏️ Collect

To start collecting, load your keypair with some ETH, and then use the `collect` command:

```sh
bitz collect
```

## ❓ Help

Add the `-h` flag on any command to pull up a help menu with documentation:

```sh
bitz -h
```

### 优化后代码说明

# 私钥准备
把私钥放在 key.txt 文件中，格式为 base58私钥 或者  [123,123,123,123] 形式的私钥 都可以

默认为 key.txt

想自定义的话 可以 --keypair 如 bitz collect --keypair key2.txt

# 运行程序
```bash
# macOS
bitz collect

# windows
bitz.exe collect 

# 以下命令以macOS为例

# 设置最小难度 为 25
bitz collect -m 25
```

# 领取代币
```bitz claim```

# 检查余额
```bitz account```
# 自定义cpu 
默认为cpu 数量 -1。比如 一共有 64 个cpu，就使用63个cpu 挖矿，剩下一个cpu 防止你电脑卡死
```bitz collect --c 8```

因为有一些函数 会因为网络情况等 报错导致程序直接退出 我没修复
```bash
#macos linux 新建一个sh 文件 执行sh 文件就行
#!/bin/bash

while true; do
  echo "启动中..."
  bitz collect
  echo "程序已退出，5秒后重启..."
  sleep 5
done


# windows 在 power shell 中运行
while(1){try{./bitz.exe collect}catch{}}
```