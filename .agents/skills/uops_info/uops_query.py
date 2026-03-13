#!/usr/bin/env python3
"""
uops_query.py — CLI tool to query uops.info instruction performance data.

Parses the uops.info instructions.xml and provides fast lookups for:
  - Instruction throughput, latency, uops, and port usage
  - Filtering by architecture, ISA extension, and instruction mnemonic

Data source: https://uops.info/xml.html
"""

import argparse
import json
import sys
import os
import xml.etree.ElementTree as ET
import urllib.request

# Architecture code → human-readable name
ARCH_NAMES = {
    "CON": "Conroe",
    "WOL": "Wolfdale",
    "NHM": "Nehalem",
    "WSM": "Westmere",
    "SNB": "Sandy Bridge",
    "IVB": "Ivy Bridge",
    "HSW": "Haswell",
    "BDW": "Broadwell",
    "SKL": "Skylake",
    "SKX": "Skylake-X",
    "KBL": "Kaby Lake",
    "CFL": "Coffee Lake",
    "CNL": "Cannon Lake",
    "CLX": "Cascade Lake",
    "ICL": "Ice Lake",
    "TGL": "Tiger Lake",
    "RKL": "Rocket Lake",
    "ADL-P": "Alder Lake-P",
    "ADL-E": "Alder Lake-E",
    "BNL": "Bonnell",
    "AMT": "Airmont",
    "GLM": "Goldmont",
    "GLP": "Goldmont Plus",
    "TRM": "Tremont",
    "ZEN+": "AMD Zen+",
    "ZEN2": "AMD Zen 2",
    "ZEN3": "AMD Zen 3",
    "ZEN4": "AMD Zen 4",
}

XML_URL = "https://uops.info/instructions.xml"

# Default XML path: same directory as this script
DEFAULT_XML = os.path.join(os.path.dirname(os.path.abspath(__file__)), "instructions.xml")


def ensure_xml(xml_path):
    """Download instructions.xml if it doesn't exist."""
    if os.path.isfile(xml_path):
        return
    print(f"Downloading {XML_URL} ...", file=sys.stderr)
    print(f"(~110 MB, this may take a minute)", file=sys.stderr)
    try:
        urllib.request.urlretrieve(XML_URL, xml_path)
        size_mb = os.path.getsize(xml_path) / (1024 * 1024)
        print(f"Downloaded {size_mb:.1f} MB to {xml_path}", file=sys.stderr)
    except Exception as e:
        print(f"Error downloading: {e}", file=sys.stderr)
        print(f"You can manually download from {XML_URL}", file=sys.stderr)
        sys.exit(1)


def parse_measurement(meas_elem):
    """Extract measurement data from a <measurement> element.

    Captures ALL attributes generically to avoid missing fields.
    Known attributes include: TP_loop, TP_unrolled, TP_ports, uops,
    uops_retire_slots, uops_MITE, uops_MS, complex_decoder, ports,
    div_cycles, macro_fusible, available_simple_decoders, and all
    *_indexed and *_same_reg variants.
    """
    # Capture all attributes generically
    result = dict(meas_elem.attrib)

    # Parse child <latency> elements
    latencies = []
    for lat in meas_elem.findall("latency"):
        latencies.append(dict(lat.attrib))
    if latencies:
        result["latencies"] = latencies

    return result


def parse_iaca(iaca_elem):
    """Extract IACA data from an <IACA> element. Captures all attributes."""
    return dict(iaca_elem.attrib)


def parse_doc(doc_elem):
    """Extract vendor documentation data from a <doc> element. Captures all attributes."""
    return dict(doc_elem.attrib)


def parse_operand(op_elem):
    """Extract operand info from an <operand> element.

    Captures ALL attributes generically. Known attributes include:
    idx, name, type, r, w, width, xtype, memory-prefix, memory-suffix,
    suppressed, implicit, conditionalWrite, VSIB, opmask, multireg,
    base, index, seg, moffs, and all flag_* attributes.
    """
    result = dict(op_elem.attrib)
    # Register list as text content
    if op_elem.text and op_elem.text.strip():
        result["registers"] = op_elem.text.strip()
    return result


def iter_instructions(xml_path, arch_filter=None, ext_filter=None):
    """
    Stream-parse the XML and yield instruction dicts.

    Each yielded dict has keys:
      asm, string, iclass, iform, isa_set, category, extension, url,
      summary, url_ref, operands, architectures
    """
    context = ET.iterparse(xml_path, events=("start", "end"))
    current_extension = None

    for event, elem in context:
        if event == "start" and elem.tag == "extension":
            current_extension = elem.get("name", "")
            if ext_filter and current_extension.upper() != ext_filter.upper():
                current_extension = None  # mark to skip
        elif event == "end" and elem.tag == "instruction":
            if current_extension is None:
                elem.clear()
                continue

            instr = {
                "asm": elem.get("asm", ""),
                "string": elem.get("string", ""),
                "iclass": elem.get("iclass", ""),
                "iform": elem.get("iform", ""),
                "isa_set": elem.get("isa-set", ""),
                "category": elem.get("category", ""),
                "extension": current_extension,
                "url": elem.get("url", ""),
                "summary": elem.get("summary", ""),
                "url_ref": elem.get("url-ref", ""),
            }

            # Operands
            instr["operands"] = [parse_operand(op) for op in elem.findall("operand")]

            # Architecture performance data
            archs = {}
            for arch_elem in elem.findall("architecture"):
                arch_name = arch_elem.get("name", "")
                if arch_filter and arch_name.upper() not in arch_filter:
                    continue
                arch_data = {}
                for meas in arch_elem.findall("measurement"):
                    arch_data["measurement"] = parse_measurement(meas)
                for iaca in arch_elem.findall("IACA"):
                    arch_data.setdefault("IACA", []).append(parse_iaca(iaca))
                for doc in arch_elem.findall("doc"):
                    arch_data["doc"] = parse_doc(doc)
                if arch_data:
                    archs[arch_name] = arch_data

            if arch_filter and not archs:
                elem.clear()
                continue

            instr["architectures"] = archs
            elem.clear()
            yield instr
        elif event == "end" and elem.tag == "extension":
            elem.clear()


def collect_extensions(xml_path):
    """Return sorted list of all extension names."""
    extensions = set()
    for event, elem in ET.iterparse(xml_path, events=("start",)):
        if elem.tag == "extension":
            name = elem.get("name", "")
            if name:
                extensions.add(name)
    return sorted(extensions)


def collect_architectures(xml_path):
    """Return sorted list of all architecture codes found in the XML."""
    archs = set()
    for event, elem in ET.iterparse(xml_path, events=("start",)):
        if elem.tag == "architecture":
            name = elem.get("name", "")
            if name:
                archs.add(name)
    return sorted(archs)


def format_latency_short(latencies):
    """Format latency list into a compact string."""
    if not latencies:
        return "-"
    # Group by (start_op, target_op)
    seen = set()
    parts = []
    for lat in latencies:
        s = lat.get("start_op", "?")
        t = lat.get("target_op", "?")
        key = (s, t)
        if key in seen:
            continue
        seen.add(key)
        if "cycles" in lat:
            parts.append(f"op{s}→op{t}: {lat['cycles']}c")
        elif "cycles_mem" in lat:
            ub = "≤" if lat.get("cycles_mem_is_upper_bound") == "1" else ""
            addr = lat.get("cycles_addr", "?")
            mem = lat.get("cycles_mem", "?")
            parts.append(f"op{s}→op{t}: addr={addr}c mem={ub}{mem}c")
    return "; ".join(parts) if parts else "-"


def print_instruction_human(instr, verbose=False):
    """Pretty-print one instruction record."""
    print(f"\n{'='*72}")
    print(f"  {instr['string']}")
    if instr.get("summary"):
        print(f"  {instr['summary']}")
    print(f"  Extension: {instr['extension']}  |  ISA Set: {instr['isa_set']}  |  Category: {instr['category']}")
    if instr.get("url"):
        print(f"  URL: https://{instr['url']}")
    print(f"{'='*72}")

    # Operands
    if instr["operands"]:
        ops = []
        for op in instr["operands"]:
            parts = []
            if op.get("type") == "reg":
                rw = ""
                if op.get("r") == "1" and op.get("w") == "1":
                    rw = "rw"
                elif op.get("r") == "1":
                    rw = "r"
                elif op.get("w") == "1":
                    rw = "w"
                parts.append(f"Reg({rw})")
                if op.get("width"):
                    parts.append(f"{op['width']}b")
            elif op.get("type") == "mem":
                rw = "r" if op.get("r") == "1" else "w" if op.get("w") == "1" else "rw"
                prefix = op.get("memory-prefix", "")
                parts.append(f"Mem({rw}) {prefix}".strip())
            elif op.get("type") == "flags":
                flag_parts = []
                for k, v in op.items():
                    if k.startswith("flag_"):
                        flag_parts.append(f"{k[5:]}={v}")
                parts.append(f"Flags({', '.join(flag_parts)})")
            elif op.get("type") == "imm":
                parts.append(f"Imm({op.get('width', '?')}b)")
            else:
                parts.append(f"{op.get('type', '?')}")
            ops.append(" ".join(parts))
        print(f"  Operands: {' | '.join(ops)}")

    # Architecture data
    if not instr["architectures"]:
        print("  (no performance data for selected architectures)")
        return

    # Header
    print(f"\n  {'Arch':<8} {'TP(loop)':>9} {'TP(unrl)':>9} {'TP(port)':>9} {'μops':>5} {'Ports':<20} {'Latency'}")
    print(f"  {'─'*7}  {'─'*9} {'─'*9} {'─'*9} {'─'*5} {'─'*20} {'─'*30}")

    for arch_name in sorted(instr["architectures"].keys(),
                            key=lambda a: list(ARCH_NAMES.keys()).index(a)
                            if a in ARCH_NAMES else 999):
        arch_data = instr["architectures"][arch_name]
        meas = arch_data.get("measurement", {})
        tp_loop = meas.get("TP_loop", "-")
        tp_unrl = meas.get("TP_unrolled", "-")
        tp_port = meas.get("TP_ports", "-")
        uops = meas.get("uops", "-")
        ports = meas.get("ports", "-")
        lats = format_latency_short(meas.get("latencies", []))

        print(f"  {arch_name:<8} {tp_loop:>9} {tp_unrl:>9} {tp_port:>9} {uops:>5} {ports:<20} {lats}")

        # Show doc data if present and verbose
        if verbose and "doc" in arch_data:
            doc = arch_data["doc"]
            doc_parts = [f"{k}={v}" for k, v in doc.items()]
            print(f"  {'':8} doc: {', '.join(doc_parts)}")


def cmd_search(args):
    """Search for instructions by mnemonic."""
    xml_path = args.file
    query = args.mnemonic
    arch_filter = None
    if args.arch:
        arch_filter = set(a.strip().upper() for a in args.arch.split(","))
    ext_filter = args.extension

    count = 0
    results = []

    for instr in iter_instructions(xml_path, arch_filter=arch_filter, ext_filter=ext_filter):
        asm = instr["asm"]
        iclass = instr["iclass"]

        if args.exact:
            if query.upper() != asm.upper() and query.upper() != iclass.upper():
                continue
        else:
            if query.upper() not in asm.upper() and query.upper() not in iclass.upper():
                continue

        count += 1

        if args.json:
            results.append(instr)
        else:
            print_instruction_human(instr, verbose=args.verbose)

    if args.json:
        json.dump(results, sys.stdout, indent=2)
        print()

    if not args.json:
        print(f"\n  Found {count} instruction form(s).")


def cmd_info(args):
    """Show detailed info for a specific instruction form (by iform)."""
    xml_path = args.file
    iform_query = args.iform

    arch_filter = None
    if args.arch:
        arch_filter = set(a.strip().upper() for a in args.arch.split(","))

    for instr in iter_instructions(xml_path, arch_filter=arch_filter):
        if instr["iform"].upper() == iform_query.upper():
            if args.json:
                json.dump(instr, sys.stdout, indent=2)
                print()
            else:
                print_instruction_human(instr, verbose=True)
            return

    print(f"No instruction found with iform '{iform_query}'.", file=sys.stderr)
    sys.exit(1)


def cmd_list_arch(args):
    """List all available architectures."""
    if args.json:
        archs = collect_architectures(args.file)
        result = [{"code": a, "name": ARCH_NAMES.get(a, a)} for a in archs]
        json.dump(result, sys.stdout, indent=2)
        print()
    else:
        print(f"\n  {'Code':<8} {'Name'}")
        print(f"  {'─'*7}  {'─'*20}")
        for code in ARCH_NAMES:
            print(f"  {code:<8} {ARCH_NAMES[code]}")
        # Also check for any unknown codes in the XML
        xml_archs = collect_architectures(args.file)
        unknown = [a for a in xml_archs if a not in ARCH_NAMES]
        if unknown:
            for code in unknown:
                print(f"  {code:<8} (unknown)")
        print(f"\n  {len(ARCH_NAMES) + len(unknown)} architecture(s)")


def cmd_list_ext(args):
    """List all ISA extensions."""
    extensions = collect_extensions(args.file)
    if args.json:
        json.dump(extensions, sys.stdout, indent=2)
        print()
    else:
        print(f"\n  ISA Extensions ({len(extensions)}):")
        print(f"  {'─'*40}")
        for ext in extensions:
            print(f"  {ext}")
        print(f"\n  {len(extensions)} extension(s)")


def main():
    parser = argparse.ArgumentParser(
        prog="uops_query",
        description="Query uops.info instruction performance data from XML.",
        formatter_class=argparse.RawDescriptionHelpFormatter,
        epilog="""\
Examples:
  %(prog)s search PREFETCHW
  %(prog)s search PREFETCHW --arch SKL,ZEN4
  %(prog)s search ADD --exact --arch ICL
  %(prog)s search VMOVDQU --extension AVX512EVEX --json
  %(prog)s info PREFETCHW_0F0Dr1
  %(prog)s list-arch
  %(prog)s list-ext
        """,
    )
    parser.add_argument("-f", "--file", default=DEFAULT_XML,
                        help=f"Path to instructions.xml (default: {DEFAULT_XML})")
    parser.add_argument("--json", action="store_true",
                        help="Output results as JSON")

    subparsers = parser.add_subparsers(dest="command", required=True)

    # search
    p_search = subparsers.add_parser("search",
                                      help="Search instructions by mnemonic")
    p_search.add_argument("mnemonic", help="Instruction mnemonic to search for")
    p_search.add_argument("--arch", help="Filter by architecture code(s), comma-separated (e.g. SKL,ZEN4)")
    p_search.add_argument("--extension", help="Filter by ISA extension (e.g. SSE2, AVX512EVEX)")
    p_search.add_argument("--exact", action="store_true",
                           help="Exact match instead of substring")
    p_search.add_argument("--verbose", "-v", action="store_true",
                           help="Show IACA and doc data")
    p_search.set_defaults(func=cmd_search)

    # info
    p_info = subparsers.add_parser("info",
                                    help="Show detailed info for a specific instruction form")
    p_info.add_argument("iform", help="Instruction form identifier (e.g. PREFETCHW_0F0Dr1)")
    p_info.add_argument("--arch", help="Filter by architecture code(s), comma-separated")
    p_info.set_defaults(func=cmd_info)

    # list-arch
    p_larch = subparsers.add_parser("list-arch", help="List all architectures")
    p_larch.set_defaults(func=cmd_list_arch)

    # list-ext
    p_lext = subparsers.add_parser("list-ext", help="List all ISA extensions")
    p_lext.set_defaults(func=cmd_list_ext)

    args = parser.parse_args()

    # Auto-download XML if not present
    ensure_xml(args.file)

    args.func(args)


if __name__ == "__main__":
    main()
