# Configuration Values

`cargo-maelstrom` supports the following [configuration values](../config.md):

Value                                                                  | Type    | Description                                                                                 | Default
-----------------------------------------------------------------------|---------|---------------------------------------------------------------------------------------------|----------------
<span style="white-space: nowrap;">`cache-size`</span>                 | string  | [target cache disk space usage](#cache-size)                                                | `"1 GB"`
<span style="white-space: nowrap;">`inline-limit`</span>               | string  | [maximum amount of captured standard output error](#inline-limit)                           | `"1 MB"`
<span style="white-space: nowrap;">`slots`</span>                      | number  | [job slots available](#slots)                                                               | 1 per CPU
<span style="white-space: nowrap;">`container-image-depot-root`</span> | string  | [container images cache directory](#container-image-depot-root)                             | `$XDG_CACHE_HOME/maelstrom/containers`
`accept-invalid-remote-container-tls-certs`                            | boolean | [allow invalid container registry certificates](#accept-invalid-remote-container-tls-certs) | `false`
<span style="white-space: nowrap;">`broker`</span>                     | string  | [address of broker](#broker)                                                                | standalone mode
<span style="white-space: nowrap;">`log-level`</span>                  | string  | [minimum log level](#log-level)                                                             | `"info"`
<span style="white-space: nowrap;">`quiet`</span>                      | boolean | [don't output per-test information](#quiet)                                                 | `false`
<span style="white-space: nowrap;">`timeout`</span>                    | string  | [override timeout value tests](#timeout)                                                    | don't override
<span style="white-space: nowrap;">`features`</span>                   | string  | [comma-separated list of features to activate](#cargo)                                      | Cargo's default
<span style="white-space: nowrap;">`all-features`</span>               | boolean | [activate all available features](#cargo)                                                   | Cargo's default
<span style="white-space: nowrap;">`no-default-features`</span>        | boolean | [do not activate the `default` feature](#cargo)                                             | Cargo's default
<span style="white-space: nowrap;">`profile`</span>                    | string  | [build artifacts with the specified profile](#cargo)                                        | Cargo's default
<span style="white-space: nowrap;">`target`</span>                     | string  | [build for the target triple](#cargo)                                                       | Cargo's default
<span style="white-space: nowrap;">`target-dir`</span>                 | string  | [directory for all generated artifacts](#cargo)                                             | Cargo's default
<span style="white-space: nowrap;">`manifest-path`</span>              | string  | [path to `Cargo.toml`](#cargo)                                                              | Cargo's default
<span style="white-space: nowrap;">`frozen`</span>                     | boolean | [require `Cargo.lock` and cache are up to date](#cargo)                                     | Cargo's default
<span style="white-space: nowrap;">`locked`</span>                     | boolean | [require `Cargo.lock` is up to date](#cargo)                                                | Cargo's default
<span style="white-space: nowrap;">`offline`</span>                    | boolean | [run without Cargo accessing the network](#cargo)                                           | Cargo's default

## `cache-size`

This is a [local-worker setting](../local-worker.md), common to all clients. See [here](../local-worker.md#cache-size) for details.

## `inline-limit`

This is a [local-worker setting](../local-worker.md), common to all clients. See [here](../local-worker.md#inline-limit) for details.

## `slots`

This is a [local-worker setting](../local-worker.md), common to all clients. See [here](../local-worker.md#slots) for details.

## `container-image-depot-root`

This is a [container-image setting](../container-images.md), common to all clients. See [here](../container-images.md#container-image-depot-root) for details.

## `accept-invalid-remote-container-tls-certs`

This is a [container-image setting](../container-images.md), common to all clients. See [here](../container-images.md#accept-invalid-remote-container-tls-certs) for details.

## `broker`

The `broker` configuration value specifies the socket address of the broker.
This configuration value is optional. If not provided, <span
style="white-space: nowrap;">`cargo-maelstrom`</span> will run in [standalone
mode](../local-worker.md).

Here are some example value socket addresses:
  - `broker.example.org:1234`
  - `192.0.2.3:1234`
  - `[2001:db8::3]:1234`

## `log-level`

This is a setting [common to all](../common-config.md) Maelstrom programs.
See [here](../common-config.md#log-level) for details.

<span style="white-space: nowrap;">`cargo-maelstrom`</span> always prints log
messages to stdout. It also passes
the log level to `maelstrom-client`, which will log its output in a [file named
`client-process.log` in the state directory](target-dir.md#client-log-file).

## `quiet`

The `quiet` configuration values, if set to `true`, causes <span
style="white-space: nowrap;">`cargo-maelstrom`</span> to be more more succinct
with its output. If <span style="white-space: nowrap;">`cargo-maelstrom`</span>
is outputting to a terminal, it will display a single-line progress bar
indicating all test state, then print a summary at the end. If not outputting
to a terminal, it will only print a summary at the end.

## `timeout`

The optional `timeout` configuration value provides the
[timeout](../spec.md#timeout) value to use for all tests. This will override
any value set in [`cargo-maelstrom.toml`](spec/fields.md#timeout).

## Cargo Settings {#cargo}

<span style="white-space: nowrap;">`cargo-maelstrom`</span> shells out to
`cargo` to get metadata about tests and to build the test artifacts. For the
former, it uses `cargo metadata`. For the latter, it uses `cargo test
--no-run`.

<span style="white-space: nowrap;">`cargo-maelstrom`</span> supports a number
of command-line options that are passed through directly to `cargo`. It does
not inspect these values at all.

Command-Line Option                                             | Cargo Grouping                                                                                  | Passed To
----------------------------------------------------------------|-------------------------------------------------------------------------------------------------|--------------------------------------------------------------------------------------------------------------------------------------------------
`features`                                                      | [feature selection](https://doc.rust-lang.org/cargo/commands/cargo-test.html#feature-selection) | [`test`](https://doc.rust-lang.org/cargo/commands/cargo-test.html) and [`metadata`](https://doc.rust-lang.org/cargo/commands/cargo-metadata.html)
<span style="white-space: nowrap;">`all-features`</span>        | [feature selection](https://doc.rust-lang.org/cargo/commands/cargo-test.html#feature-selection) | [`test`](https://doc.rust-lang.org/cargo/commands/cargo-test.html) and [`metadata`](https://doc.rust-lang.org/cargo/commands/cargo-metadata.html)
<span style="white-space: nowrap;">`no-default-features`</span> | [feature selection](https://doc.rust-lang.org/cargo/commands/cargo-test.html#feature-selection) | [`test`](https://doc.rust-lang.org/cargo/commands/cargo-test.html) and [`metadata`](https://doc.rust-lang.org/cargo/commands/cargo-metadata.html)
`profile`                                                       | [compilation](https://doc.rust-lang.org/cargo/commands/cargo-test.html#compilation-options)     | [`test`](https://doc.rust-lang.org/cargo/commands/cargo-test.html)
`target`                                                        | [compilation](https://doc.rust-lang.org/cargo/commands/cargo-test.html#compilation-options)     | [`test`](https://doc.rust-lang.org/cargo/commands/cargo-test.html)
<span style="white-space: nowrap;">`target-dir`</span>          | [output](https://doc.rust-lang.org/cargo/commands/cargo-test.html#output-options)               | [`test`](https://doc.rust-lang.org/cargo/commands/cargo-test.html)
<span style="white-space: nowrap;">`manifest-path`</span>       | [manifest](https://doc.rust-lang.org/cargo/commands/cargo-test.html#manifest-options)           | [`test`](https://doc.rust-lang.org/cargo/commands/cargo-test.html) and [`metadata`](https://doc.rust-lang.org/cargo/commands/cargo-metadata.html)
`frozen`                                                        | [manifest](https://doc.rust-lang.org/cargo/commands/cargo-test.html#manifest-options)           | [`test`](https://doc.rust-lang.org/cargo/commands/cargo-test.html) and [`metadata`](https://doc.rust-lang.org/cargo/commands/cargo-metadata.html)
`locked`                                                        | [manifest](https://doc.rust-lang.org/cargo/commands/cargo-test.html#manifest-options)           | [`test`](https://doc.rust-lang.org/cargo/commands/cargo-test.html) and [`metadata`](https://doc.rust-lang.org/cargo/commands/cargo-metadata.html)
`offline`                                                       | [manifest](https://doc.rust-lang.org/cargo/commands/cargo-test.html#manifest-options)           | [`test`](https://doc.rust-lang.org/cargo/commands/cargo-test.html) and [`metadata`](https://doc.rust-lang.org/cargo/commands/cargo-metadata.html)

<span style="white-space: nowrap;">`cargo-maelstrom`</span> doesn't accept
multiple instances of the `--features` command-line option. Instead, combine
the features into a single, comma-separated argument like this:
`--features=feat1,feat2,feat3`.

<span style="white-space: nowrap;">`cargo-maelstrom`</span> doesn't accept the
`--release` alias. Use `--profile=release` instead.
