# Contributing

Thank you for your interest in contributing to Rufin! This document has some simple guidelines for contributing.

# Commits

For commit names and PRs, you may use [Conventional Commits](https://www.conventionalcommits.org/en/v1.0.0/#summary). This is not enforced, but is preferrable.

# Translations

Each language lives in one file: `po/<locale>.po`. To start a translation, copy `po/rufin.pot` to a new locale file, a full locale id like `tr_TR.po`, `de_DE.po`, or `pt_BR.po`, set `Language: locale_id \n` and translate  `msgstr ""` values.
