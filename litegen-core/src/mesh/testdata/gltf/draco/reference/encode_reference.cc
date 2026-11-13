// encode_reference: one Draco stream of the reference corpus, written by
// Google's own encoder library (mesh-gaps item 2, Task 4b; README.md beside
// this file).
//
// litegen's code, built against google/draco at
// d429de84b52a860705507da0884764c5d8321423 (1.5.9 and one commit, which
// changes only a JavaScript example), whose library is Apache-2.0.
//
// draco_encoder reads OBJ and PLY, which carry no tangents and no generic
// attributes (Ruling B35), so this tool takes a primitive of every attribute
// from the geometry file reference.py writes, and encodes it as draco_encoder
// would: the same option calls (draco_encoder.cc:337-359 at that commit),
// through an ExpertEncoder, as draco_encoder does, after deduplicating values
// and points, as draco's own loaders and its JavaScript encoder do.
//
// Usage: encode_reference <geometry.geom> <out.drc> <setting>
//
// <setting> is one of:
//   cl<C>-qp<Q>     draco_encoder -cl C -qp Q, its other defaults kept:
//                   -qt 10, -qn 8, -qg 8 (speed 10 - C).
//   seqc-qp<Q>      -cl 0 -qp Q (sequential), with the global option
//                   compress_connectivity, which draco_encoder never sets.
//   swap-qp<Q>      -cl 10 -qp Q, with the EdgeBreaker coder the encoder's
//                   tiny-mesh rule doesn't pick (mesh_edgebreaker_encoder.cc:
//                   standard under 1,000 faces): valence under 1,000 faces,
//                   standard from 1,000 on.
//   meta-qp<Q>      -cl 7 -qp Q, with geometry and attribute metadata, as
//                   draco_encoder --metadata writes some.
//
// The geometry file (little-endian): "LGGEOM01", then u32 points, u32 faces,
// u32 attributes; 3 u32 point indices a face; then each attribute: u8 Draco
// attribute type, u8 Draco data type, u8 components, u8 normalized, u8 name
// length and the name (its glTF semantic), and points x components values.

#include <cstdint>
#include <cstdio>
#include <cstdlib>
#include <cstring>
#include <fstream>
#include <iterator>
#include <memory>
#include <string>
#include <vector>

#include "draco/compression/config/compression_shared.h"
#include "draco/compression/encode.h"
#include "draco/compression/expert_encode.h"
#include "draco/core/draco_types.h"
#include "draco/core/encoder_buffer.h"
#include "draco/mesh/mesh.h"
#include "draco/metadata/geometry_metadata.h"

namespace {

[[noreturn]] void Fail(const std::string& what) {
  std::fprintf(stderr, "encode_reference: %s\n", what.c_str());
  std::exit(1);
}

struct Attribute {
  draco::GeometryAttribute::Type type;
  draco::DataType data_type;
  int components;
  bool normalized;
  std::string name;
  std::vector<uint8_t> values;
};

struct Geometry {
  uint32_t num_points = 0;
  std::vector<uint32_t> indices;
  std::vector<Attribute> attributes;
};

class Reader {
 public:
  explicit Reader(std::vector<uint8_t> bytes) : bytes_(std::move(bytes)) {}
  const uint8_t* Take(size_t n) {
    if (n > bytes_.size() - at_) Fail("the geometry file is cut short");
    const uint8_t* p = bytes_.data() + at_;
    at_ += n;
    return p;
  }
  uint8_t U8() { return *Take(1); }
  uint32_t U32() {
    const uint8_t* p = Take(4);
    return uint32_t{p[0]} | uint32_t{p[1]} << 8 | uint32_t{p[2]} << 16 |
           uint32_t{p[3]} << 24;
  }
  bool AtEnd() const { return at_ == bytes_.size(); }

 private:
  std::vector<uint8_t> bytes_;
  size_t at_ = 0;
};

std::vector<uint8_t> ReadFile(const char* path) {
  std::ifstream file(path, std::ios::binary);
  if (!file) Fail(std::string("can't open ") + path);
  return std::vector<uint8_t>(std::istreambuf_iterator<char>(file), {});
}

Geometry ReadGeometry(const char* path) {
  Reader r(ReadFile(path));
  if (std::memcmp(r.Take(8), "LGGEOM01", 8) != 0) Fail("not a geometry file");
  Geometry g;
  g.num_points = r.U32();
  const uint32_t num_faces = r.U32();
  const uint32_t num_attributes = r.U32();
  g.indices.resize(size_t{num_faces} * 3);
  for (uint32_t& i : g.indices) {
    i = r.U32();
    if (i >= g.num_points) Fail("a face names a point past the points");
  }
  for (uint32_t a = 0; a < num_attributes; ++a) {
    Attribute att;
    const uint8_t type = r.U8();
    if (type > draco::GeometryAttribute::GENERIC) Fail("an attribute type");
    att.type = static_cast<draco::GeometryAttribute::Type>(type);
    att.data_type = static_cast<draco::DataType>(r.U8());
    const int32_t size = draco::DataTypeLength(att.data_type);
    if (size <= 0) Fail("an attribute data type");
    att.components = r.U8();
    if (att.components < 1 || att.components > 4) Fail("a component count");
    att.normalized = r.U8() != 0;
    const uint8_t name_length = r.U8();
    const uint8_t* name = r.Take(name_length);
    att.name.assign(reinterpret_cast<const char*>(name), name_length);
    const size_t bytes = size_t{g.num_points} * att.components * size;
    const uint8_t* values = r.Take(bytes);
    att.values.assign(values, values + bytes);
    g.attributes.push_back(std::move(att));
  }
  if (!r.AtEnd()) Fail("trailing bytes after the attributes");
  return g;
}

// One value a point for every attribute, then values and points deduplicated,
// as draco's own loaders (obj_decoder.cc:278-282, ply_decoder.cc:82-88) and
// its JavaScript encoder (encoder_webidl_wrapper.cc:266-269) do before
// encoding: a position shared by several points becomes one value, so the
// EdgeBreaker coder sees the connectivity and the attribute seams.
std::unique_ptr<draco::Mesh> BuildMesh(const Geometry& g) {
  auto mesh = std::make_unique<draco::Mesh>();
  mesh->set_num_points(g.num_points);
  const size_t num_faces = g.indices.size() / 3;
  mesh->SetNumFaces(num_faces);
  for (size_t f = 0; f < num_faces; ++f) {
    draco::Mesh::Face face;
    for (int c = 0; c < 3; ++c) face[c] = draco::PointIndex(g.indices[3 * f + c]);
    mesh->SetFace(draco::FaceIndex(static_cast<uint32_t>(f)), face);
  }
  for (const Attribute& a : g.attributes) {
    const int64_t stride = int64_t{a.components} * draco::DataTypeLength(a.data_type);
    draco::GeometryAttribute ga;
    ga.Init(a.type, nullptr, static_cast<uint8_t>(a.components), a.data_type,
            a.normalized, stride, 0);
    const int id = mesh->AddAttribute(ga, /*identity_mapping=*/true, g.num_points);
    if (id < 0) Fail("AddAttribute refused " + a.name);
    draco::PointAttribute* pa = mesh->attribute(id);
    for (uint32_t p = 0; p < g.num_points; ++p) {
      pa->SetAttributeValue(draco::AttributeValueIndex(p), a.values.data() + p * stride);
    }
  }
  if (!mesh->DeduplicateAttributeValues()) Fail("DeduplicateAttributeValues failed");
  mesh->DeduplicatePointIds();
  return mesh;
}

// Geometry metadata of each entry type Draco has (a string, an int, an int
// array, a double, nested metadata), and a "name" entry for every attribute,
// its glTF semantic.
void AddMetadata(draco::Mesh* mesh, const Geometry& g, const std::string& input) {
  auto metadata = std::make_unique<draco::GeometryMetadata>();
  metadata->AddEntryString("name", input);
  metadata->AddEntryInt("faces", static_cast<int32_t>(mesh->num_faces()));
  metadata->AddEntryIntArray("counts", {static_cast<int32_t>(mesh->num_points()),
                                        static_cast<int32_t>(mesh->num_faces())});
  metadata->AddEntryDouble("scale", 0.25);
  auto generator = std::make_unique<draco::Metadata>();
  generator->AddEntryString("tool", "encode_reference");
  if (!metadata->AddSubMetadata("generator", std::move(generator))) Fail("AddSubMetadata");
  mesh->AddMetadata(std::move(metadata));
  for (int i = 0; i < mesh->num_attributes(); ++i) {
    auto attribute = std::make_unique<draco::AttributeMetadata>();
    attribute->AddEntryString("name", g.attributes[i].name);
    mesh->AddAttributeMetadata(i, std::move(attribute));
  }
}

struct Setting {
  int compression_level = -1;
  int position_bits = -1;
  bool compress_connectivity = false;
  bool swap_coder = false;
  bool metadata = false;
};

Setting ParseSetting(const std::string& tag) {
  Setting s;
  const size_t qp = tag.find("-qp");
  if (qp == std::string::npos) Fail("setting " + tag);
  const std::string head = tag.substr(0, qp);
  s.position_bits = std::atoi(tag.c_str() + qp + 3);
  if (head.rfind("cl", 0) == 0) {
    s.compression_level = std::atoi(head.c_str() + 2);
  } else if (head == "seqc") {
    s.compression_level = 0;
    s.compress_connectivity = true;
  } else if (head == "swap") {
    s.compression_level = 10;
    s.swap_coder = true;
  } else if (head == "meta") {
    s.compression_level = 7;
    s.metadata = true;
  } else {
    Fail("setting " + tag);
  }
  if (s.compression_level < 0 || s.compression_level > 10) Fail("compression level in " + tag);
  if (s.position_bits < 1 || s.position_bits > 30) Fail("position bits in " + tag);
  return s;
}

std::string Stem(const std::string& path) {
  const size_t slash = path.find_last_of('/');
  std::string name = slash == std::string::npos ? path : path.substr(slash + 1);
  const size_t dot = name.rfind(".geom");
  return dot == std::string::npos ? name : name.substr(0, dot);
}

}  // namespace

int main(int argc, char** argv) {
  const uint16_t probe = 1;
  if (*reinterpret_cast<const uint8_t*>(&probe) != 1) Fail("a big-endian host");
  if (argc != 4) Fail("usage: encode_reference <geometry.geom> <out.drc> <setting>");
  const Geometry g = ReadGeometry(argv[1]);
  const Setting s = ParseSetting(argv[3]);
  std::unique_ptr<draco::Mesh> mesh = BuildMesh(g);
  if (s.metadata) AddMetadata(mesh.get(), g, Stem(argv[1]));

  // draco_encoder's own calls (draco_encoder.cc:337-359), its defaults for
  // everything but the compression level and the position bits.
  const int speed = 10 - s.compression_level;
  draco::Encoder encoder;
  encoder.SetAttributeQuantization(draco::GeometryAttribute::POSITION, s.position_bits);
  encoder.SetAttributeQuantization(draco::GeometryAttribute::TEX_COORD, 10);
  encoder.SetAttributeQuantization(draco::GeometryAttribute::NORMAL, 8);
  encoder.SetAttributeQuantization(draco::GeometryAttribute::GENERIC, 8);
  encoder.SetSpeedOptions(speed, speed);
  draco::ExpertEncoder expert(*mesh);
  expert.Reset(encoder.CreateExpertEncoderOptions(*mesh));
  if (s.compress_connectivity) expert.options().SetGlobalBool("compress_connectivity", true);
  if (s.swap_coder) {
    expert.options().SetGlobalInt("edgebreaker_method",
                                  mesh->num_faces() < 1000
                                      ? draco::MESH_EDGEBREAKER_VALENCE_ENCODING
                                      : draco::MESH_EDGEBREAKER_STANDARD_ENCODING);
  }

  draco::EncoderBuffer buffer;
  const draco::Status status = expert.EncodeToBuffer(&buffer);
  if (!status.ok()) Fail(std::string("encoding failed: ") + status.error_msg());
  std::ofstream out(argv[2], std::ios::binary);
  out.write(buffer.data(), static_cast<std::streamsize>(buffer.size()));
  if (!out) Fail(std::string("can't write ") + argv[2]);
  std::printf("%s %s: %u points, %u faces, %zu bytes\n", Stem(argv[1]).c_str(), argv[3],
              mesh->num_points(), mesh->num_faces(), buffer.size());
  return 0;
}
