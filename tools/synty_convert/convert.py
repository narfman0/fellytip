# Synty FBX -> GLB single-file converter for Fellytip.
# Called once per file by convert.ps1.
#
# Usage:
#   blender.exe --background --python tools/synty_convert/convert.py -- <src.fbx> <dst.glb>

import bpy
import os
import sys


def parse_args():
    argv = sys.argv
    try:
        sep = argv.index("--")
    except ValueError:
        print("ERROR: pass -- <src.fbx> <dst.glb>")
        sys.exit(1)
    rest = argv[sep + 1:]
    if len(rest) < 2:
        print("ERROR: expected <src.fbx> <dst.glb>")
        sys.exit(1)
    return rest[0], rest[1]


def main():
    src_path, dst_path = parse_args()

    bpy.ops.wm.read_factory_settings(use_empty=True)

    result = bpy.ops.import_scene.fbx(
        filepath=src_path,
        use_anim=False,
        ignore_leaf_bones=True,
        force_connect_children=False,
    )
    if 'FINISHED' not in result:
        print(f"IMPORT_FAILED: {src_path}")
        sys.exit(2)

    os.makedirs(os.path.dirname(dst_path), exist_ok=True)

    result = bpy.ops.export_scene.gltf(
        filepath=dst_path,
        export_format='GLB',
        export_apply=True,
        export_yup=True,
        export_materials='EXPORT',
        export_animations=False,
    )
    if 'FINISHED' not in result:
        print(f"EXPORT_FAILED: {dst_path}")
        sys.exit(3)

    print(f"OK: {dst_path}")


main()
