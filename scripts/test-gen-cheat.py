#!/usr/bin/env python3
"""Regression tests for scripts/gen-cheat.py's sheet parsing (scripts/test-gen-cheat.py).

Fixtures are cut from the real nova and socat sheets of cheat/cheatsheets at the pinned
commit: nova covers backslash continuations, socat covers YAML front matter, consecutive
comment lines before a command, and comment lines between commands.
"""
import os
import sys

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import importlib.util
spec = importlib.util.spec_from_file_location("gen_cheat", os.path.join(os.path.dirname(os.path.abspath(__file__)), "gen-cheat.py"))
gen_cheat = importlib.util.module_from_spec(spec)
spec.loader.exec_module(gen_cheat)
parse_sheet = gen_cheat.parse_sheet

NOVA = """\
# To list VMs on current tenant:
nova list

# To boot a VM on a specific host:
nova boot --nic net-id=<net_id> \\
          --image <image_id> \\
          --flavor <flavor> \\
          --availability-zone nova:<host_name> <vm_name>

# To stop a server:
nova stop <server>
"""

SOCAT = """\
---
tags: [ networking ]
---
# socat connect to http-server (port 80 on 'butzel.info')
socat TCP4:butzel.info:80 -

# practical examples:

# complete real working http-example:
#  (sleep is necessary to prevent socat closing socket before data received)
(echo -e "GET / HTTP/1.1\\r\\nHost: butzel.info\\r\\n\\r" && sleep 1) \\
| socat tcp4:butzel.info:80 -

# listener for above reverse shell (on remote.butzel.info):
socat file:`tty`,raw,echo=0 tcp-listen:3180
# or: nc -lp 3180
"""

failures = []


def check(name, actual, expected):
    if actual != expected:
        failures.append(f"{name}:\n  expected {expected!r}\n  got      {actual!r}")


check("front matter stripped, plain block", parse_sheet(SOCAT)[:1], [
    ("socat connect to http-server (port 80 on 'butzel.info')", "socat TCP4:butzel.info:80 -"),
])
check("comment between commands does not change the description or add an example",
      [entry for entry in parse_sheet(SOCAT) if entry[1].startswith("(echo") or entry[1].startswith("socat file")], [
    ("complete real working http-example:",
     '(echo -e "GET / HTTP/1.1\\r\\nHost: butzel.info\\r\\n\\r" && sleep 1) | socat tcp4:butzel.info:80 -'),
    ("listener for above reverse shell (on remote.butzel.info):", "socat file:`tty`,raw,echo=0 tcp-listen:3180"),
])
check("continuation lines join into one example with a single space", parse_sheet(NOVA)[1:2], [
    ("To boot a VM on a specific host:",
     "nova boot --nic net-id=<net_id> --image <image_id> --flavor <flavor> --availability-zone nova:<host_name> <vm_name>"),
])
check("one example per command line, descriptions from the first comment of each block",
      parse_sheet(NOVA)[0:1] + parse_sheet(NOVA)[2:3], [
    ("To list VMs on current tenant:", "nova list"),
    ("To stop a server:", "nova stop <server>"),
])

if failures:
    print("\n\n".join(failures))
    sys.exit(1)
print("test-gen-cheat: all parsing checks passed")
