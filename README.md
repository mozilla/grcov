# grcov

[![Build Status](https://travis-ci.com/mozilla/grcov.svg?branch=master)](https://travis-ci.com/mozilla/grcov)
[![Build status](https://ci.appveyor.com/api/projects/status/1957u00h26alxey2/branch/master?svg=true)](https://ci.appveyor.com/project/marco-c/grcov)
[![codecov](https://codecov.io/gh/mozilla/grcov/branch/master/graph/badge.svg)](https://codecov.io/gh/mozilla/grcov)
[![crates.io](https://img.shields.io/crates/v/grcov.svg)](https://crates.io/crates/grcov)

grcov collects and aggregates code coverage information for multiple source files.
grcov processes .profraw and .gcda files which can be generated from llvm/clang or gcc.
grcov also processes lcov files (for JS coverage) and JaCoCo files (for Java coverage).
Linux, macOS and Windows are supported.

This is a project initiated by Mozilla to gather code coverage results on Firefox.

<!-- omit in toc -->
## Table of Contents

- [man grcov](#man-grcov)
- [How to get grcov](#how-to-get-grcov)
- [Usage](#usage)
  - [Example: How to generate source-based coverage for a Rust project](#example-how-to-generate-source-based-coverage-for-a-rust-project)
  - [Example: How to generate .gcda files for C/C++](#example-how-to-generate-gcda-files-for-cc)
  - [Example: How to generate .gcda files for a Rust project](#example-how-to-generate-gcda-files-for-a-rust-project)
  - [Generate a coverage report from coverage artifacts](#generate-a-coverage-report-from-coverage-artifacts)
    - [LCOV output](#lcov-output)
    - [Coveralls/Codecov output](#coverallscodecov-output)
    - [grcov with Travis](#grcov-with-travis)
  - [Alternative reports](#alternative-reports)
  - [Hosting HTML reports and using coverage badges](#hosting-html-reports-and-using-coverage-badges)
    - [Example](#example)
  - [Enabling symlinks on Windows](#enabling-symlinks-on-windows)
- [Auto-formatting](#auto-formatting)
- [Build & Test](#build--test)
- [Minimum requirements](#minimum-requirements)
- [License](#license)

## man grcov

```text
USAGE:
    grcov [FLAGS] [OPTIONS] <paths>...

FLAGS:
        --branch
            Enables parsing branch coverage information

        --guess-directory-when-missing


    -h, --help
            Prints help information

        --ignore-not-existing
            Ignore source files that can't be found on the disk

        --llvm
            Speeds-up parsing, when the code coverage information is exclusively coming from a llvm build

        --parallel
            Sets the build type to be parallel for 'coveralls' and 'coveralls+' formats

    -V, --version
            Prints version information


OPTIONS:
    -b, --binary-path <PATH>
            Sets the path to the directory containing the compiled binaries to be used

        --commit-sha <COMMIT HASH>
            Sets the hash of the commit used to generate the code coverage data

        --excl-br-line <regex>
            Lines in covered files containing this marker will be excluded from branch coverage.

        --excl-br-start <regex>
            Marks the beginning of a section excluded from branch coverage. The current line is part of this section.

        --excl-br-stop <regex>
            Marks the end of a section excluded from branch coverage. The current line is part of this section.

        --excl-line <regex>
            Lines in covered files containing this marker will be excluded.

        --excl-start <regex>
            Marks the beginning of an excluded section. The current line is part of this section.

        --excl-stop <regex>
            Marks the end of an excluded section. The current line is part of this section.

        --filter <filter>
            Filters out covered/uncovered files. Use 'covered' to only return covered files, 'uncovered' to only return
            uncovered files [possible values: covered, uncovered]
        --ignore <PATH>...
            Ignore files/directories specified as globs

        --keep-only <PATH>...
            Keep only files/directories specified as globs

        --log <LOG>
            Set the file where to log (or stderr or stdout). Defaults to 'stderr' [default: stderr]

    -o, --output-path <PATH>
            Specifies the output path

    -t, --output-type <OUTPUT TYPE>
            Sets a custom output type:
            - *html* for a HTML coverage report;
            - *coveralls* for the Coveralls specific format;
            - *lcov* for the lcov INFO format;
            - *covdir* for the covdir recursive JSON format;
            - *coveralls+* for the Coveralls specific format with function information;
            - *ade* for the ActiveData-ETL specific format;
            - *cobertura* for a cobertura coverage report;
            - *files* to only return a list of files.
             [default: lcov]  [possible values: ade, lcov, coveralls, coveralls+, files, covdir, html, cobertura]
        --path-mapping <PATH>...


    -p, --prefix-dir <PATH>
            Specifies a prefix to remove from the paths (e.g. if grcov is run on a different machine than the one that
            generated the code coverage information)
        --service-job-id <SERVICE JOB ID>
            Sets the service job id [aliases: service-job-number]

        --service-name <SERVICE NAME>
            Sets the service name

        --service-number <SERVICE NUMBER>
            Sets the service number

        --service-pull-request <SERVICE PULL REQUEST>
            Sets the service pull request number

    -s, --source-dir <DIRECTORY>
            Specifies the root directory of the source files

        --threads <NUMBER>
             [default: 11]

        --token <TOKEN>
            Sets the repository token from Coveralls, required for the 'coveralls' and 'coveralls+' formats

        --vcs-branch <VCS BRANCH>
            Set the branch for coveralls report. Defaults to 'master' [default: master]


ARGS:
    <paths>...
            Sets the input paths to use
```

## How to get grcov

Grcov can be downloaded from [releases](https://github.com/mozilla/grcov/releases) or, if you have Rust installed,
you can run `cargo install grcov`.

## Usage

**Nightly Rust is required** to use grcov for Rust coverage. Alternatively, you can `export
RUSTC_BOOTSTRAP=1`, which basically turns your stable rustc into a Nightly one.

### Example: How to generate source-based coverage for a Rust project

1. Install the llvm-tools or llvm-tools-preview component:

   ```sh
   rustup component add llvm-tools-preview
   ```

2. Ensure that the following environment variable is set up:

   ```sh
   export RUSTFLAGS="-Zinstrument-coverage"
   ```

3. Build your code:

   `cargo build`

4. Ensure each test runs gets its own profile information by defining the LLVM_PROFILE_FILE environment variable (%p will be replaced by the process ID, and %m by the binary signature):

   ```sh
   export LLVM_PROFILE_FILE="your_name-%p-%m.profraw"
   ```

5. Run your tests:

   `cargo test`

In the CWD, you will see a `.profraw` file has been generated. This contains the profiling information that grcov will parse, alongside with your binaries.

### Example: How to generate .gcda files for C/C++

Pass `--coverage` to `clang` or `gcc` (or for older gcc versions pass `-ftest-coverage` and `-fprofile-arcs` options (see [gcc docs](https://gcc.gnu.org/onlinedocs/gcc/Gcov-Data-Files.html)).

### Example: How to generate .gcda files for a Rust project

1. Ensure that the following environment variables are set up:

   ```sh
   export CARGO_INCREMENTAL=0
   export RUSTFLAGS="-Zprofile -Ccodegen-units=1 -Copt-level=0 -Clink-dead-code -Coverflow-checks=off -Zpanic_abort_tests -Cpanic=abort"
   export RUSTDOCFLAGS="-Cpanic=abort"
   ```

   These will ensure that things like dead code elimination do not skew the coverage.

2. Build your code:

   `cargo build`

   If you look in `target/debug/deps` dir you will see `.gcno` files have appeared. These are the locations that could be covered.

3. Run your tests:

   `cargo test`

   In the `target/debug/deps/` dir you will now also see `.gcda` files. These contain the hit counts on which of those locations have been reached. Both sets of files are used as inputs to `grcov`.

### Generate a coverage report from coverage artifacts

Generate a html coverage report like this:

```sh
grcov . -s . --binary-path ./target/debug/ -t html --branch --ignore-not-existing -o ./target/debug/coverage/
```

N.B.: The `--binary-path` argument is only necessary for source-based coverage.

You can see the report in `target/debug/coverage/index.html`.

(or alternatively with `-t lcov` grcov will output a lcov compatible coverage report that you could then feed into lcov's `genhtml` command).

#### LCOV output

By passing `-t lcov` you could generate an lcov.info file and pass it to genhtml:

```sh
genhtml -o ./target/debug/coverage/ --show-details --highlight --ignore-errors source --legend ./target/debug/lcov.info
```

LCOV output should be used when uploading to Codecov, with the `--branch` argument for branch coverage support.

#### Coveralls output

Coverage can also be generated in coveralls format:

```sh
grcov . --binary-path ./target/debug/ -t coveralls -s . --token YOUR_COVERALLS_TOKEN > coveralls.json
```

#### grcov with Travis

Here is an example of .travis.yml file for source-based coverage:

```yaml
language: rust

before_install:
  - curl -L https://github.com/mozilla/grcov/releases/latest/download/grcov-linux-x86_64.tar.bz2 | tar jxf -

matrix:
  include:
    - os: linux
      rust: nightly

script:
    - rustup component add llvm-tools-preview
    - export RUSTFLAGS="-Zinstrument-coverage"
    - cargo build --verbose
    - LLVM_PROFILE_FILE="your_name-%p-%m.profraw" cargo test --verbose
    - ./grcov . --binary-path ./target/debug/ -s . -t lcov --branch --ignore-not-existing --ignore "/*" -o lcov.info
    - bash <(curl -s https://codecov.io/bash) -f lcov.info
```

Here is an example of .travis.yml file:

```yaml
language: rust

before_install:
  - curl -L https://github.com/mozilla/grcov/releases/latest/download/grcov-linux-x86_64.tar.bz2 | tar jxf -

matrix:
  include:
    - os: linux
      rust: nightly

script:
    - export CARGO_INCREMENTAL=0
    - export RUSTFLAGS="-Zprofile -Ccodegen-units=1 -Copt-level=0 -Clink-dead-code -Coverflow-checks=off -Zpanic_abort_tests -Cpanic=abort"
    - export RUSTDOCFLAGS="-Cpanic=abort"
    - cargo build --verbose $CARGO_OPTIONS
    - cargo test --verbose $CARGO_OPTIONS
    - |
      zip -0 ccov.zip `find . \( -name "YOUR_PROJECT_NAME*.gc*" \) -print`;
      ./grcov ccov.zip -s . -t lcov --llvm --branch --ignore-not-existing --ignore "/*" -o lcov.info;
      bash <(curl -s https://codecov.io/bash) -f lcov.info;
```

### Alternative reports

grcov provides the following output types:

| Output Type `-t` | Description                                                               |
| ---------------- | ------------------------------------------------------------------------- |
| lcov (default)   | lcov's INFO format that is compatible with the linux coverage project.    |
| ade              | ActiveData\-ETL format. Only useful for Mozilla projects.                 |
| coveralls        | Generates coverage in Coveralls format.                                   |
| coveralls+       | Like coveralls but with function level information.                       |
| files            | Output a file list of covered or uncovered source files.                  |
| covdir           | Provides coverage in a recursive JSON format.                             |
| html             | Output a HTML coverage report, including coverage badges for your README. |

### Hosting HTML reports and using coverage badges

The HTML report can be hosted on static website providers like GitHub Pages, Netlify and others. It
is common to provide a coverage badge in a project's readme to show the current percentage of
covered code.

To still allow adding the badge when using a static site host, grcov generates coverage badges and
a JSON file with coverage information that can be used with <https://shields.io> to dynamically
generate badges.

The coverage data for <htttps://shields.io> can be found at `/coverage.json` and the generated
bagdes are available as SVGs at `/badges/*svg`.

The design of generated badges is taken from `shields.io` but may not be updated immediately if there
is any change. Using their endpoint method is recommended if other badges from their service are
used already.

### Enabling symlinks on Windows

`grcov` uses symbolic links to avoid copying files, when processing directories
of coverage data. On Windows, by default, creating symbolic links to files
requires Administrator privileges. (The reason is to avoid security attacks in
applications that were designed before Windows added support for symbolic
links.)

When running on Windows `grcov` will attempt to create a symbolic link. If that
fails then `grcov` will fall back to copying the file. Copying is less efficient
but at least allows users to run `grcov`. `grcov` will also print a warning
when it falls back to copying a file, advising the user either to enable the
privilege for their account or to run as Administrator.

You can enable the "Create Symbolic Links" privilege for your account so that
you do not need to run as Administrator to use `grcov`.

1. Click Start, then select "Local Group Policy Editor". Or just run
   `gpedit.msc` to open it directly.
1. In the navigation tree, select "Computer Configuration", "Windows Settings",
   "Security Settings", "Local Policies".
1. In the pane on the right, select "Create symbolic links" and double-click it.
1. Click "Add User or Group", and add your account.
1. Log out and then log back in.

#### Example

Let's consider we have a project at with username `sample` and project `awesome` that is hosted with
GitHub Pages at `https://sample.github.io/awesome`.

By using the the `shields.io` endpoint we can create a Markdown badge like so:

```md
[![coverage](https://shields.io/endpoint?url=https://sample.github.io/awesome/coverage.json)](https://sample.github.io/awesome/index.html)
```

If we want to avoid using `shields.io` as well, we can use the generated badges as follows (note
the different URL for the image):

```md
[![coverage](https://sample.github.io/awesome/badges/flat.svg)](https://sample.github.io/awesome/index.html)
```

## Auto-formatting

This project is using pre-commit. Please run `pre-commit install` to install the git pre-commit hooks on your clone. Instructions on how to install pre-commit can be found [here](https://pre-commit.com/#install).

Every time you will try to commit, pre-commit will run checks on your files to make sure they follow our style standards and they aren't affected by some simple issues. If the checks fail, pre-commit won't let you commit.

## Build & Test

Build with:

```sh
cargo build
```

To run unit tests:

```sh
cargo test --lib
```

To run integration tests, it is suggested to use the Docker image defined in tests/Dockerfile. Simply build the image to run them:

```sh
docker build -t marcocas/grcov -f tests/Dockerfile .
```

Otherwise, if you don't want to use Docker, the only prerequisite is to install GCC 7, setting the `GCC_CXX` environment variable to `g++-7` and the `GCOV` environment variable to `gcov-7`. Then run the tests with:

```sh
cargo test
```

## Minimum requirements

- GCC 4.9 or higher is required (if parsing coverage artifacts generated by GCC).
- Rust 1.52

## License

Published under the MPL 2.0 license.
