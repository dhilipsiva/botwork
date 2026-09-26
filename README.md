# botwork

botwork is a single-binary, generic and open-source automation framework written in Rust for acceptance testing, acceptance test driven development (ATDD), and robotic process automation (RPA). The syntax is basically plain text (in any human lanuage) with parameters. Easily extendible with Rust, Python & JavaScript. An efficient, fast alternative to Robot Framework.

# Why botwork?

I have been using RobotFramework for a couple of years now. While it is a super-awesome framework, there are a couple of things that I am not very fond of:

1. It basically requires Python (and virtualenv) to run. This means, it needs more space (when building container images, for instance) and consumes a lot of memory (Python is the love of my life, but it is slow and resource-heavy).
2. The syntax could have been even more simpler. For instance, two (or more) space token seperator, `${}`, `@{}`, etc. variable usage confuses people who are new to the framework.
3. It is mostly extendible only with Python.

I wanted:

1. An efficient, fast, single-binary tool.
1. An even more simpler syntax than RobotFramework.
1. Extendible with Rust, Python (via PyO3), JavaScript (via neon), etc
1. Proper language defnition with PEG parser.
1. LSP & TreeSitter Support. 
1. Most of all, to have fun building something that I can introduce to my kids.


# Getting Started

1. Clone the repo
2. run `cargo run -- --file examples/02-syntaxes.botwork`

`Log` writes readable values to stdout. Diagnostics go to stderr, and uncaught
script errors return a nonzero exit status. Add `--debug` after `cargo run --`
to trace top-level statement locations on stderr. See [language behavior](docs/language.md)
and [testing instructions](docs/testing.md) for the implemented contracts.

Or you can create a file from below sample and pass the path to cargo run


# Sample Code

Here is what a botwork script might looke like right now

```botwork
# Declaration
What is square-root of |number| divided by |divisor| equals, eh?!... {
	|square| = |number ^ 2|
	Return |square/divisor| 
	Log |"This statement will never execute"|
}

# Invocation (case-insensitive)
|answer| = WHAT is    sQuAre-RoOt of |6| divided by|2|equals, EH?!...

Log |"Here is your answer:"|
Log |answer|
```

# Roadmap to version 1.0

botwork is just taking its baby steps. There are so many things that are still missing and it goes without saying the the syntax & apis will change any time. Not to mention the hacy code that I managed to get working over the weekend. The Idea is to let it out in the wild and see if people are interested in a tool like this. 

If there is interest out there for a tool like botwork, I plan to dedicate more time to make v1.0 happen. So here is a bunch of things that needs to be done before botwork can be tagged v1.0:

- [ ] Basic syntax
  - [x] Statements
  - [x] If condition
  - [x] For & While loop
  - [x] Basic arithmatic and logical operations
  - [x] Datatypes: int, float, string, bool, array, map
  - [x] Try/Catch
  - [x] Custom statements
  - [ ] Map and Array access 
  - [ ] Imports (other botwork files, wasm files, packages; locally or from URL)
- [ ] Docs
  - [x] README
  - [ ] Getting started docs
  - [ ] Syntax docs
  - [ ] Statement docs
- [ ] More statements out-of-box (like the ones RobotFramework Offers)
  - [ ] [Built-ins](http://robotframework.org/robotframework/latest/libraries/BuiltIn.html)
  - [ ] [Collections](http://robotframework.org/robotframework/latest/libraries/Collections.html)
  - [ ] [Datetime](http://robotframework.org/robotframework/latest/libraries/DateTime.html)
  - [ ] [Operation System](http://robotframework.org/robotframework/latest/libraries/OperatingSystem.html)
  - [ ] [Process](http://robotframework.org/robotframework/latest/libraries/Process.html)
  - [ ] [String](http://robotframework.org/robotframework/latest/libraries/String.html)
  - [ ] Making HTTP Requests
- [ ] integrations
  - [ ] Selenium/Webdriver
  - [ ] Appium
  - [ ] Playwirght
- [ ] Reports
  - [ ] Console Reports
  - [ ] JSON Reports
  - [ ] HTML Reports
  - [ ] Report listeners
- [ ] Tooling
  - [ ] LSP support
  - [ ] Editor support (Mainly Helix/Vim/VS Code)
  - [ ] TreeSitter grammar
  - [ ] Linting  
  - [ ] Trusted & Verified registry (for botwork packages)
  - [ ] Python extention support (via pyo3)
  - [ ] Javascript extention support (via neon)
  - [ ] WASM extention support (via WASI)
- [ ] CLI
  - [ ] Ability to pass variables from CLI / Files
  - [ ] Run in parallel
- [ ] Fully async, non-blocking operations
- [ ] Refactor my crappy code :P 
- [ ] Unit-tests with atleast 50% coverage 
