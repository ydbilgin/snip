#!/usr/bin/env python3
"""Inspect a built Windows candidate without executing it; no third-party packages."""

import argparse
import hashlib
import json
from pathlib import Path
import struct
import xml.etree.ElementTree as ET


def inspect(path, manifest_out=None, require_static_crt=False):
    data = path.read_bytes()

    def u16(at):
        return struct.unpack_from('<H', data, at)[0]

    def u32(at):
        return struct.unpack_from('<I', data, at)[0]

    assert data[:2] == b'MZ', 'missing DOS header'
    pe = u32(0x3C)
    assert data[pe:pe + 4] == b'PE\0\0', 'missing PE signature'
    assert u16(pe + 4) == 0x8664, 'candidate must be x86_64'
    opt = pe + 24
    assert u16(opt) == 0x20B, 'candidate must be PE32+'
    assert u16(opt + 68) == 2, 'candidate must use the Windows GUI subsystem'
    sections = []
    for i in range(u16(pe + 6)):
        at = opt + u16(pe + 20) + i * 40
        sections.append((u32(at + 12), max(u32(at + 8), u32(at + 16)), u32(at + 20)))

    def offset(rva):
        for start, size, raw in sections:
            if start <= rva < start + size:
                return raw + rva - start
        raise ValueError(f'RVA outside sections: {rva:#x}')

    imports = []
    import_rva = u32(opt + 112 + 8)
    if import_rva:
        at = offset(import_rva)
        while any(data[at:at + 20]):
            name = offset(u32(at + 12))
            imports.append(data[name:data.index(0, name)].decode('ascii'))
            at += 20

    resource_rva = u32(opt + 112 + 16)
    assert resource_rva, 'missing PE resources'
    base = offset(resource_rva)

    def entries(relative):
        at = base + relative
        count = u16(at + 12) + u16(at + 14)
        return [(u32(at + 16 + i * 8), u32(at + 20 + i * 8)) for i in range(count)]

    def directory(relative, wanted):
        for name, value in entries(relative):
            if name == wanted:
                assert value & 0x80000000, 'expected resource directory'
                return value & 0x7FFFFFFF
        raise ValueError(f'missing resource id {wanted}')

    # RT_MANIFEST (24), application manifest ID 1, then every language variant.
    manifest_dir = directory(directory(0, 24), 1)
    manifests = []
    for language, entry in entries(manifest_dir):
        assert not entry & 0x80000000, 'expected manifest resource data'
        at = base + entry
        raw, size = offset(u32(at)), u32(at + 4)
        manifest = data[raw:raw + size].rstrip(b'\0')
        xml = ET.fromstring(manifest)
        dpi = xml.find('.//{http://schemas.microsoft.com/SMI/2016/WindowsSettings}dpiAwareness')
        privilege = xml.find('.//{urn:schemas-microsoft-com:asm.v3}requestedExecutionLevel')
        assert dpi is not None and dpi.text == 'PerMonitorV2', 'missing PerMonitorV2'
        assert privilege is not None and privilege.get('level') == 'asInvoker', 'unexpected elevation'
        assert privilege.get('uiAccess') == 'false', 'unexpected uiAccess'
        manifests.append({'language': language, 'bytes': size, 'dpi': dpi.text, 'level': privilege.get('level')})
        if manifest_out is not None:
            manifest_out.write_bytes(manifest)
    assert manifests, 'empty manifest language directory'

    crt = [name for name in imports if name.lower().startswith(
        ('vcruntime', 'msvcp', 'api-ms-win-crt', 'libgcc', 'libstdc++', 'libwinpthread'))
        or name.lower() in ('ucrtbase.dll', 'msvcrt.dll')]
    if require_static_crt:
        assert not crt, f'dynamic CRT imports: {crt}'
    return {
        'file': path.name, 'bytes': len(data), 'sha256': hashlib.sha256(data).hexdigest(),
        'machine': 'x86_64 (0x8664)', 'format': 'PE32+', 'subsystem': 'Windows GUI',
        'manifest_type': 24, 'manifest_id': 1, 'manifests': manifests,
        'imports': sorted(imports), 'dynamic_crt_imports': crt,
        'debug_directory_bytes': u32(opt + 112 + 6 * 8 + 4),
        'executed': False,
    }


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('exe', type=Path)
    parser.add_argument('--manifest-out', type=Path)
    parser.add_argument('--require-static-crt', action='store_true')
    args = parser.parse_args()
    print(json.dumps(inspect(args.exe, args.manifest_out, args.require_static_crt), indent=2))
