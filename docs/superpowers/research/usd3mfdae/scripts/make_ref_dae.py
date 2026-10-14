#!/usr/bin/env python3
"""Hand-built reference COLLADA 1.4.1 files mirroring the glTF test asset.

ref.dae   : textured + vertex colours + 2 materials (multi <triangles>), skin, matrix animation
morph.dae : skin over a morph controller (glTF morph target) + weights animation
"""
import math, os, sys

out = sys.argv[1]
os.makedirs(out, exist_ok=True)
c = s = math.sqrt(0.5)

def fl(vals):
    return ' '.join(('%.7g' % v) for v in vals)

def mat_rows(m):  # m given as 4 rows -> COLLADA row-major text
    return fl([x for r in m for x in r])

I4 = [[1, 0, 0, 0], [0, 1, 0, 0], [0, 0, 1, 0], [0, 0, 0, 1]]
T01 = [[1, 0, 0, 0], [0, 1, 0, 1], [0, 0, 1, 0], [0, 0, 0, 1]]
RZ45_T01 = [[c, -s, 0, 0], [s, c, 0, 1], [0, 0, 1, 0], [0, 0, 0, 1]]
IBM1 = [[1, 0, 0, 0], [0, 1, 0, -1], [0, 0, 1, 0], [0, 0, 0, 1]]  # inverse of T(0,1,0)

# merged vertex arrays: strip (6) + red triangle (3)
P = [(-.5, 0, 0), (.5, 0, 0), (-.5, 1, 0), (.5, 1, 0), (-.5, 2, 0), (.5, 2, 0), (1, 0, 0), (2, 0, 0), (1.5, 1, 0)]
UV_GLTF = [(0, 1), (1, 1), (0, .5), (1, .5), (0, 0), (1, 0)]
UV = [(u, 1 - v) for u, v in UV_GLTF] + [(0, 0)] * 3          # COLLADA: V up
COL = [(1, 0, 0, 1), (0, 1, 0, 1), (0, 0, 1, 1), (1, 1, 0, 1), (1, 0, 1, 1), (0, 1, 1, 1)] + [(1, 1, 1, 1)] * 3
# (joint, weight) per vertex
INFL = [[(0, 1)], [(0, 1)], [(0, .5), (1, .5)], [(0, .5), (1, .5)], [(1, 1)], [(1, 1)], [(0, 1)], [(0, 1)], [(0, 1)]]
weights = sorted({w for vi in INFL for _, w in vi})
v_list, vcount = [], []
for vi in INFL:
    vcount.append(len(vi))
    for j, w in vi:
        v_list += [j, weights.index(w)]
flat = lambda l: [x for t in l for x in t]

def geometry(gid):
    return f'''    <geometry id="{gid}" name="StripMesh">
      <mesh>
        <source id="{gid}-pos">
          <float_array id="{gid}-pos-array" count="{len(P) * 3}">{fl(flat(P))}</float_array>
          <technique_common>
            <accessor source="#{gid}-pos-array" count="{len(P)}" stride="3">
              <param name="X" type="float"/><param name="Y" type="float"/><param name="Z" type="float"/>
            </accessor>
          </technique_common>
        </source>
        <source id="{gid}-nrm">
          <float_array id="{gid}-nrm-array" count="3">0 0 1</float_array>
          <technique_common>
            <accessor source="#{gid}-nrm-array" count="1" stride="3">
              <param name="X" type="float"/><param name="Y" type="float"/><param name="Z" type="float"/>
            </accessor>
          </technique_common>
        </source>
        <source id="{gid}-uv">
          <float_array id="{gid}-uv-array" count="{len(UV) * 2}">{fl(flat(UV))}</float_array>
          <technique_common>
            <accessor source="#{gid}-uv-array" count="{len(UV)}" stride="2">
              <param name="S" type="float"/><param name="T" type="float"/>
            </accessor>
          </technique_common>
        </source>
        <source id="{gid}-col">
          <float_array id="{gid}-col-array" count="{len(COL) * 4}">{fl(flat(COL))}</float_array>
          <technique_common>
            <accessor source="#{gid}-col-array" count="{len(COL)}" stride="4">
              <param name="R" type="float"/><param name="G" type="float"/><param name="B" type="float"/><param name="A" type="float"/>
            </accessor>
          </technique_common>
        </source>
        <vertices id="{gid}-vtx">
          <input semantic="POSITION" source="#{gid}-pos"/>
        </vertices>
        <triangles count="4" material="TexturedMat-sym">
          <input semantic="VERTEX" source="#{gid}-vtx" offset="0"/>
          <input semantic="NORMAL" source="#{gid}-nrm" offset="1"/>
          <input semantic="TEXCOORD" source="#{gid}-uv" offset="0" set="0"/>
          <input semantic="COLOR" source="#{gid}-col" offset="0" set="0"/>
          <p>{' '.join('%d 0' % i for i in [0, 1, 3, 0, 3, 2, 2, 3, 5, 2, 5, 4])}</p>
        </triangles>
        <triangles count="1" material="RedMat-sym">
          <input semantic="VERTEX" source="#{gid}-vtx" offset="0"/>
          <input semantic="NORMAL" source="#{gid}-nrm" offset="1"/>
          <p>6 0 7 0 8 0</p>
        </triangles>
      </mesh>
    </geometry>'''

def skin(cid, source):
    return f'''    <controller id="{cid}" name="StripSkin">
      <skin source="#{source}">
        <bind_shape_matrix>{mat_rows(I4)}</bind_shape_matrix>
        <source id="{cid}-joints">
          <Name_array id="{cid}-joints-array" count="2">Bone0 Bone1</Name_array>
          <technique_common>
            <accessor source="#{cid}-joints-array" count="2" stride="1"><param name="JOINT" type="name"/></accessor>
          </technique_common>
        </source>
        <source id="{cid}-ibm">
          <float_array id="{cid}-ibm-array" count="32">{mat_rows(I4)} {mat_rows(IBM1)}</float_array>
          <technique_common>
            <accessor source="#{cid}-ibm-array" count="2" stride="16"><param name="TRANSFORM" type="float4x4"/></accessor>
          </technique_common>
        </source>
        <source id="{cid}-w">
          <float_array id="{cid}-w-array" count="{len(weights)}">{fl(weights)}</float_array>
          <technique_common>
            <accessor source="#{cid}-w-array" count="{len(weights)}" stride="1"><param name="WEIGHT" type="float"/></accessor>
          </technique_common>
        </source>
        <joints>
          <input semantic="JOINT" source="#{cid}-joints"/>
          <input semantic="INV_BIND_MATRIX" source="#{cid}-ibm"/>
        </joints>
        <vertex_weights count="{len(INFL)}">
          <input semantic="JOINT" source="#{cid}-joints" offset="0"/>
          <input semantic="WEIGHT" source="#{cid}-w" offset="1"/>
          <vcount>{' '.join(map(str, vcount))}</vcount>
          <v>{' '.join(map(str, v_list))}</v>
        </vertex_weights>
      </skin>
    </controller>'''

def anim_matrix(aid, target, times, mats, interp):
    n = len(times)
    return f'''    <animation id="{aid}">
      <source id="{aid}-in">
        <float_array id="{aid}-in-array" count="{n}">{fl(times)}</float_array>
        <technique_common>
          <accessor source="#{aid}-in-array" count="{n}" stride="1"><param name="TIME" type="float"/></accessor>
        </technique_common>
      </source>
      <source id="{aid}-out">
        <float_array id="{aid}-out-array" count="{16 * n}">{' '.join(mat_rows(m) for m in mats)}</float_array>
        <technique_common>
          <accessor source="#{aid}-out-array" count="{n}" stride="16"><param name="TRANSFORM" type="float4x4"/></accessor>
        </technique_common>
      </source>
      <source id="{aid}-interp">
        <Name_array id="{aid}-interp-array" count="{n}">{' '.join([interp] * n)}</Name_array>
        <technique_common>
          <accessor source="#{aid}-interp-array" count="{n}" stride="1"><param name="INTERPOLATION" type="name"/></accessor>
        </technique_common>
      </source>
      <sampler id="{aid}-sampler">
        <input semantic="INPUT" source="#{aid}-in"/>
        <input semantic="OUTPUT" source="#{aid}-out"/>
        <input semantic="INTERPOLATION" source="#{aid}-interp"/>
      </sampler>
      <channel source="#{aid}-sampler" target="{target}"/>
    </animation>'''

EFFECTS = '''  <library_images>
    <image id="basecolor-img" name="basecolor"><init_from>basecolor.png</init_from></image>
  </library_images>
  <library_effects>
    <effect id="TexturedMat-fx">
      <profile_COMMON>
        <newparam sid="basecolor-surface">
          <surface type="2D"><init_from>basecolor-img</init_from></surface>
        </newparam>
        <newparam sid="basecolor-sampler">
          <sampler2D><source>basecolor-surface</source></sampler2D>
        </newparam>
        <technique sid="common">
          <lambert>
            <emission><color sid="emission">0 0 0 1</color></emission>
            <diffuse><texture texture="basecolor-sampler" texcoord="UVSET0"/></diffuse>
            <transparent opaque="A_ONE"><color>1 1 1 1</color></transparent>
            <transparency><float sid="transparency">1</float></transparency>
          </lambert>
          <extra>
            <technique profile="GOOGLEEARTH"><double_sided>1</double_sided></technique>
          </extra>
        </technique>
      </profile_COMMON>
    </effect>
    <effect id="RedMat-fx">
      <profile_COMMON>
        <technique sid="common">
          <phong>
            <emission><color sid="emission">0 0 0 1</color></emission>
            <diffuse><color sid="diffuse">1 0 0 1</color></diffuse>
            <specular><color sid="specular">0.04 0.04 0.04 1</color></specular>
            <shininess><float sid="shininess">20</float></shininess>
          </phong>
        </technique>
      </profile_COMMON>
    </effect>
  </library_effects>
  <library_materials>
    <material id="TexturedMat-mat" name="TexturedMat"><instance_effect url="#TexturedMat-fx"/></material>
    <material id="RedMat-mat" name="RedMat"><instance_effect url="#RedMat-fx"/></material>
  </library_materials>'''

def doc(controllers, extra_anims, skin_id):
    root_anim = anim_matrix('Root-anim', 'Root/transform', [0, 2], [I4, [[1, 0, 0, 1], [0, 1, 0, 0], [0, 0, 1, 0], [0, 0, 0, 1]]], 'LINEAR')
    bone_anim = anim_matrix('Bone1-anim', 'Bone1/transform', [0, 1, 2], [T01, RZ45_T01, T01], 'LINEAR')
    scale_anim = anim_matrix('Bone0-anim', 'Bone0/transform', [0, 1], [I4, [[1.2, 0, 0, 0], [0, 1.2, 0, 0], [0, 0, 1.2, 0], [0, 0, 0, 1]]], 'STEP')
    return f'''<?xml version="1.0" encoding="utf-8"?>
<COLLADA xmlns="http://www.collada.org/2005/11/COLLADASchema" version="1.4.1">
  <asset>
    <contributor><authoring_tool>make_ref_dae.py</authoring_tool></contributor>
    <created>2026-10-12T00:00:00</created>
    <modified>2026-10-12T00:00:00</modified>
    <unit name="meter" meter="1"/>
    <up_axis>Y_UP</up_axis>
  </asset>
{EFFECTS}
  <library_geometries>
{geometry('StripMesh-geo')}
  </library_geometries>
  <library_controllers>
{controllers}
  </library_controllers>
  <library_animations>
{root_anim}
{bone_anim}
{scale_anim}
{extra_anims}
  </library_animations>
  <library_visual_scenes>
    <visual_scene id="Scene" name="Scene">
      <node id="Root" name="Root" type="NODE">
        <matrix sid="transform">{mat_rows(I4)}</matrix>
        <node id="Strip" name="Strip" type="NODE">
          <matrix sid="transform">{mat_rows(I4)}</matrix>
          <instance_controller url="#{skin_id}">
            <skeleton>#Bone0</skeleton>
            <bind_material>
              <technique_common>
                <instance_material symbol="TexturedMat-sym" target="#TexturedMat-mat">
                  <bind_vertex_input semantic="UVSET0" input_semantic="TEXCOORD" input_set="0"/>
                </instance_material>
                <instance_material symbol="RedMat-sym" target="#RedMat-mat"/>
              </technique_common>
            </bind_material>
          </instance_controller>
        </node>
        <node id="Bone0" name="Bone0" sid="Bone0" type="JOINT">
          <matrix sid="transform">{mat_rows(I4)}</matrix>
          <node id="Bone1" name="Bone1" sid="Bone1" type="JOINT">
            <matrix sid="transform">{mat_rows(T01)}</matrix>
          </node>
        </node>
      </node>
    </visual_scene>
  </library_visual_scenes>
  <scene>
    <instance_visual_scene url="#Scene"/>
  </scene>
</COLLADA>
'''

open(os.path.join(out, 'ref.dae'), 'w').write(doc(skin('Strip-skin', 'StripMesh-geo'), '', 'Strip-skin'))

# morph variant: target geometry = top verts moved +x 0.5
P_T = list(P); P_T[4] = (0, 2, 0); P_T[5] = (1, 2, 0)
morph_target = f'''    <geometry id="StripMesh-target0" name="StripMesh-target0">
      <mesh>
        <source id="t0-pos">
          <float_array id="t0-pos-array" count="{len(P_T) * 3}">{fl(flat(P_T))}</float_array>
          <technique_common>
            <accessor source="#t0-pos-array" count="{len(P_T)}" stride="3">
              <param name="X" type="float"/><param name="Y" type="float"/><param name="Z" type="float"/>
            </accessor>
          </technique_common>
        </source>
        <vertices id="t0-vtx"><input semantic="POSITION" source="#t0-pos"/></vertices>
      </mesh>
    </geometry>'''
morph = '''    <controller id="Strip-morph" name="StripMorph">
      <morph source="#StripMesh-geo" method="NORMALIZED">
        <source id="Strip-morph-targets">
          <IDREF_array id="Strip-morph-targets-array" count="1">StripMesh-target0</IDREF_array>
          <technique_common>
            <accessor source="#Strip-morph-targets-array" count="1" stride="1"><param name="IDREF" type="IDREF"/></accessor>
          </technique_common>
        </source>
        <source id="Strip-morph-weights">
          <float_array id="Strip-morph-weights-array" count="1">0</float_array>
          <technique_common>
            <accessor source="#Strip-morph-weights-array" count="1" stride="1"><param name="MORPH_WEIGHT" type="float"/></accessor>
          </technique_common>
        </source>
        <targets>
          <input semantic="MORPH_TARGET" source="#Strip-morph-targets"/>
          <input semantic="MORPH_WEIGHT" source="#Strip-morph-weights"/>
        </targets>
      </morph>
    </controller>'''
weights_anim = '''    <animation id="Strip-morph-anim">
      <source id="mw-in">
        <float_array id="mw-in-array" count="3">0 1 2</float_array>
        <technique_common><accessor source="#mw-in-array" count="3" stride="1"><param name="TIME" type="float"/></accessor></technique_common>
      </source>
      <source id="mw-out">
        <float_array id="mw-out-array" count="3">0 1 0</float_array>
        <technique_common><accessor source="#mw-out-array" count="3" stride="1"><param name="WEIGHT" type="float"/></accessor></technique_common>
      </source>
      <sampler id="mw-sampler">
        <input semantic="INPUT" source="#mw-in"/>
        <input semantic="OUTPUT" source="#mw-out"/>
      </sampler>
      <channel source="#mw-sampler" target="Strip-morph-weights-array(0)"/>
    </animation>'''
d = doc(morph + '\n' + skin('Strip-skin', 'Strip-morph'), weights_anim, 'Strip-skin')
d = d.replace('  </library_geometries>', morph_target + '\n  </library_geometries>')
open(os.path.join(out, 'morph.dae'), 'w').write(d)
print('ok')
