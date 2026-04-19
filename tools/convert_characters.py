"""
Batch-convert Kenney Animated Characters Bundle FBX files to GLB.

Each character model is combined with all animation FBX files (each as an NLA
track), then exported as a single GLB with embedded animations.

Usage (from workspace root):
    blender --background --python tools/convert_characters.py

Prerequisites:
- Blender 4.x installed and on PATH (or adjust BLENDER_BIN below)
- Kenney AIO pack at C:/Users/narfman0/Desktop/kenney_aio/
- Output written to crates/client/assets/characters/

The exported GLB files embed all animations.  In Bevy 0.18 you can load them as:
    asset_server.load("characters/characterMedium.glb#Animation0")
    asset_server.load("characters/characterMedium.glb#Animation1")
    ...

The animation index order matches the order of ANIMATION_FILES below.
"""

import bpy
import os
import sys

# ── Configuration ──────────────────────────────────────────────────────────────

KENNEY_BASE = r"C:\Users\narfman0\Desktop\kenney_aio\3D assets\Animated Characters Bundle"
OUTPUT_DIR  = os.path.join(
    os.path.dirname(os.path.dirname(os.path.abspath(__file__))),
    "crates", "client", "assets", "characters",
)

SKINS_DIR = os.path.join(KENNEY_BASE, "Skins")

# Skin PNG applied to each model before export (embedded in the GLB).
SKIN_MAP = {
    "characterMedium":      "fantasyMaleA.png",
    "characterLargeMale":   "fantasyMaleB.png",
    "characterLargeFemale": "fantasyFemaleA.png",
    "characterSmall":       "casualMaleA.png",
}

CHARACTER_MODELS = [
    "characterMedium.fbx",
    "characterLargeMale.fbx",
    "characterLargeFemale.fbx",
    "characterSmall.fbx",
]

# Animations in the desired output order (index == #AnimationN in Bevy).
ANIMATION_FILES = [
    "idle.fbx",
    "walk.fbx",
    "run.fbx",
    "attack.fbx",
    "punch.fbx",
    "kick.fbx",
    "jump.fbx",
    "death.fbx",
    "crouch.fbx",
    "crouchIdle.fbx",
    "crouchWalk.fbx",
    "interactStanding.fbx",
    "interactGround.fbx",
    "shoot.fbx",
]

MODELS_DIR     = os.path.join(KENNEY_BASE, "Models")
ANIMATIONS_DIR = os.path.join(KENNEY_BASE, "Animations")

# ── Conversion ────────────────────────────────────────────────────────────────

os.makedirs(OUTPUT_DIR, exist_ok=True)

for model_file in CHARACTER_MODELS:
    model_path = os.path.join(MODELS_DIR, model_file)
    if not os.path.exists(model_path):
        print(f"[SKIP] model not found: {model_path}")
        continue

    stem = os.path.splitext(model_file)[0]
    out_path = os.path.join(OUTPUT_DIR, stem + ".glb")

    print(f"\n=== Converting {model_file} ===")

    # Start with a completely empty scene.
    bpy.ops.wm.read_factory_settings(use_empty=True)

    # Import the base character mesh + rig.
    print(f"  Importing base mesh: {model_path}")
    bpy.ops.import_scene.fbx(filepath=model_path)

    # Apply skin texture to all mesh materials so it's embedded in the GLB.
    skin_path = os.path.join(SKINS_DIR, SKIN_MAP.get(stem, "fantasyMaleA.png"))
    if os.path.exists(skin_path):
        print(f"  Applying skin: {skin_path}")
        img = bpy.data.images.load(skin_path)
        for obj in bpy.context.scene.objects:
            if obj.type != "MESH":
                continue
            for mat in obj.data.materials:
                if not mat or not mat.use_nodes:
                    continue
                bsdf = mat.node_tree.nodes.get("Principled BSDF")
                if bsdf:
                    tex = mat.node_tree.nodes.new("ShaderNodeTexImage")
                    tex.image = img
                    mat.node_tree.links.new(
                        bsdf.inputs["Base Color"], tex.outputs["Color"]
                    )
                    bsdf.inputs["Alpha"].default_value = 1.0
                mat.blend_method = "OPAQUE"
    else:
        print(f"  [WARN] Skin not found: {skin_path}")

    # Find the armature object — animations will be imported onto it.
    armature = next(
        (o for o in bpy.context.scene.objects if o.type == "ARMATURE"),
        None
    )
    if armature is None:
        print(f"  [WARN] No armature found in {model_file} — skipping animations")
    else:
        bpy.context.view_layer.objects.active = armature

        for anim_idx, anim_file in enumerate(ANIMATION_FILES):
            anim_path = os.path.join(ANIMATIONS_DIR, anim_file)
            if not os.path.exists(anim_path):
                print(f"  [SKIP] animation not found: {anim_path}")
                continue

            anim_name = os.path.splitext(anim_file)[0]
            print(f"  Importing animation: {anim_name}")

            # Snapshot scene objects and actions before import.
            before_objects = set(bpy.data.objects.keys())
            before_actions = set(bpy.data.actions.keys())

            bpy.ops.import_scene.fbx(
                filepath=anim_path,
                use_anim=True,
                ignore_leaf_bones=True,
                automatic_bone_orientation=False,
            )

            # Delete every object the FBX import created — they are duplicate
            # rigs/meshes we don't need; we only want the action it brought in.
            for obj_name in list(set(bpy.data.objects.keys()) - before_objects):
                obj = bpy.data.objects.get(obj_name)
                if obj:
                    bpy.data.objects.remove(obj, do_unlink=True)

            # Find the newly imported action, skipping binding-pose extras.
            new_actions = set(bpy.data.actions.keys()) - before_actions
            action = None
            for aname in sorted(new_actions):
                if "Targeting Pose" not in aname:
                    action = bpy.data.actions[aname]
                    break
            if action is None:
                print(f"  [WARN] No usable action found after importing {anim_name}")
                continue

            # Prefix with zero-padded index so the GLTF exporter keeps the
            # intended order (Blender sorts NLA strips alphabetically on export).
            action.name = f"{anim_idx:02d}_{anim_name}"

            # Push action to an NLA track so it's included in the GLB export.
            if armature.animation_data is None:
                armature.animation_data_create()
            track = armature.animation_data.nla_tracks.new()
            track.name = action.name
            track.strips.new(action.name, int(action.frame_range[0]), action)

        # Clear the active action so we export via NLA strips, not just one action.
        armature.animation_data.action = None

        # Remove NLA tracks the FBX importer auto-pushed (binding/targeting poses).
        import re
        bad_tracks = [
            track for track in armature.animation_data.nla_tracks
            if not any(re.match(r'^\d{2}_', strip.name) for strip in track.strips)
        ]
        for track in bad_tracks:
            armature.animation_data.nla_tracks.remove(track)

        # Also purge from bpy.data.actions — Blender 5.1's GLTF exporter exports
        # all actions referencing the armature, not just those in NLA tracks.
        for action_name in list(bpy.data.actions.keys()):
            if not re.match(r'^\d{2}_', action_name):
                bpy.data.actions.remove(bpy.data.actions[action_name])

    # Export as GLB with all NLA strip animations embedded.
    print(f"  Exporting → {out_path}")
    bpy.ops.export_scene.gltf(
        filepath=out_path,
        export_format="GLB",
        export_animations=True,
        export_nla_strips=True,
        export_image_format="AUTO",
        export_skins=True,
        export_morph=True,
        use_selection=False,
    )
    print(f"  Done: {out_path}")

print("\n=== All characters converted ===")
print(f"Output directory: {OUTPUT_DIR}")
print()
print("Bevy 0.18 usage:")
print('  asset_server.load("characters/characterMedium.glb#Scene0")    // mesh')
print('  asset_server.load("characters/characterMedium.glb#Animation0") // idle')
print('  asset_server.load("characters/characterMedium.glb#Animation1") // walk')
print('  asset_server.load("characters/characterMedium.glb#Animation2") // run')
print('  asset_server.load("characters/characterMedium.glb#Animation3") // attack')
