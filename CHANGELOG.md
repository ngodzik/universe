# Changelog

Versions follow [semantic versioning](https://semver.org). While the version is 0.x universe is an experiment: anything may change or disappear from one version to the next, commands, keys, views, settings and file formats included, without a way back.

## 0.1.0 (2026-11-08)

The first version: a project, or a system of several repositories, read with deterministic tools only and drawn in space. It is experimental and barely tested in real use, on Linux mostly, and whatever it does may change or be removed in the next version.

- The project's files as stars gathered into galaxies, grouped by module, language, layer or role.
- Its plan, its services and its database in 3D, in the same space, each also flat; its API routes flat, with what serves and calls each.
- Its deployments, every environment at once.
- Its files, opened read only, their names followed to where they are defined.

The binaries are built for Linux (x86_64, glibc 2.35 or later) and macOS (Apple Silicon). On Linux the window needs Wayland's client library, which desktops ship (`libwayland-client0` on Debian and Ubuntu). On macOS the binary is not signed: `xattr -d com.apple.quarantine universe` lets it open.
