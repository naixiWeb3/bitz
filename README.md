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