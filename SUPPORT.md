# Getting help

| You want to | Go to |
|---|---|
| Install or build Helion | [Getting started](https://helion-fpga.github.io/helion/start.html) |
| Learn the IDE and the CLI | [User guide](https://helion-fpga.github.io/helion/use.html) |
| Find a page in the docs | [Documentation index](https://helion-fpga.github.io/helion/wiki.html) |
| Ask a question or share an idea | [GitHub Discussions](https://github.com/helion-fpga/helion/discussions) |
| Report a bug or request a feature | [Issues](https://github.com/helion-fpga/helion/issues/new/choose) |
| Report a security issue | [SECURITY.md](SECURITY.md) (privately, please) |

## Before you ask

- Run `helion-ide --doctor` (or `helion doctor`) and include its output.
- Check that the gold still holds on your machine:
  `helion-ide --headless examples/counter.sv` should print `WNS_PS=9640`.
- Search existing issues and discussions.

## Scope

Helion is a community project; help is best effort. We support Helion
itself, not vendor tools (Vivado, Quartus, Radiant and so on).

Where the project is going: [ROADMAP.md](ROADMAP.md) and the
[milestones](https://github.com/helion-fpga/helion/milestones).
