# Hisoka
A Windows DNS middleware to add hosts Wildcard.

## Build
```sh
cargo build --release
```
Then compile `installer.iss` with [Inno Setup](https://jrsoftware.org/isinfo.php) in `/target/installer`.

## Test
Testing the core configuration with the CLI
```sh
cargo test -p hisoka-core
```

Setup the development folder as the environment hisoka will run inside of
```sh
mkdir dev
$env:HISOKA_HOME = "$PWD\dev"
```

Now you can run the Hisoka Service
```sh
cargo run -p hisoka-service -- --console
```

Then you can use the Hisoka CLI
```sh
# Check the logs
cargo run -p hisoka -- logs
# Manage the wildcard configuration
cargo run -p hisoka -- wildcard add dev "*.test" 127.0.0.1
cargo run -p hisoka -- wildcard add stg "*.staging.test" 10.0.0.5
cargo run -p hisoka -- wildcard add dup "*.test" 1.1.1.1      # should fail: same pattern
cargo run -p hisoka -- wildcard set stg --address 10.0.0.6
cargo run -p hisoka -- wildcard list
```