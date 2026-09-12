---
name: widget-naming
description: A widget part number is never reused or renumbered once it has shipped
metadata:
  kind: critical
  scopes:
  - widgets
  source: user
  created: 2026-01-05T11:30:00Z
---
# Widget part numbers are immutable

A widget part number is never reused or renumbered once it has shipped; a
changed part gets the next free number.

Downstream drawings cite part numbers by value, so renumbering silently
retargets every drawing that cited the old one.
