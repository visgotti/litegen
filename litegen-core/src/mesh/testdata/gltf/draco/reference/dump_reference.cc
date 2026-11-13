// dump_reference: Google's own decoding of one Draco stream, kept raw, for the
// reference manifest (mesh-gaps item 2, Task 4b; README.md beside this file).
//
// litegen's code, built against google/draco at
// d429de84b52a860705507da0884764c5d8321423 (1.5.9 and one commit, which
// changes only a JavaScript example), whose library is Apache-2.0.
//
// It decodes as draco_decoder does (draco_decoder.cc:96-97,
// Decoder::DecodeMeshFromBuffer, every attribute transform applied), and
// writes what draco_decoder's text outputs would round (Ruling B35):
//
//   <out_dir>/connectivity.bin    every face's three point indices, u32
//                                 little-endian, in the decoder's face order;
//   <out_dir>/attribute-<id>.bin  for the attribute of unique id <id>, every
//                                 point's value through the attribute's
//                                 point-to-value map, point by point, its
//                                 components at the width of its data type,
//                                 little-endian, as the decoder stores them.
//
// and prints, one per line:
//   points <n>
//   faces <n>
//   attribute <index> <unique id> <type> <data type> <components>
//             <normalized> <byte stride> <byte offset> <values> <identity map>
//   metadata <name entry, or -> <attribute metadata entries>
//
// Usage: dump_reference <in.drc> <out_dir>

#include <cinttypes>
#include <cstdint>
#include <cstdio>
#include <cstdlib>
#include <fstream>
#include <iterator>
#include <memory>
#include <string>
#include <vector>

#include "draco/compression/decode.h"
#include "draco/core/decoder_buffer.h"
#include "draco/core/draco_types.h"
#include "draco/mesh/mesh.h"
#include "draco/metadata/geometry_metadata.h"

namespace {

[[noreturn]] void Fail(const std::string& what) {
  std::fprintf(stderr, "dump_reference: %s\n", what.c_str());
  std::exit(1);
}

void Write(const std::string& path, const std::vector<uint8_t>& bytes) {
  std::ofstream out(path, std::ios::binary);
  out.write(reinterpret_cast<const char*>(bytes.data()),
            static_cast<std::streamsize>(bytes.size()));
  if (!out) Fail("can't write " + path);
}

void AppendU32(std::vector<uint8_t>* out, uint32_t v) {
  for (int i = 0; i < 4; ++i) out->push_back(static_cast<uint8_t>(v >> (8 * i)));
}

}  // namespace

int main(int argc, char** argv) {
  const uint16_t probe = 1;
  if (*reinterpret_cast<const uint8_t*>(&probe) != 1) Fail("a big-endian host");
  if (argc != 3) Fail("usage: dump_reference <in.drc> <out_dir>");
  std::ifstream file(argv[1], std::ios::binary);
  if (!file) Fail(std::string("can't open ") + argv[1]);
  const std::vector<char> data((std::istreambuf_iterator<char>(file)), {});
  const std::string dir = argv[2];

  draco::DecoderBuffer buffer;
  buffer.Init(data.data(), data.size());
  draco::Decoder decoder;
  auto maybe_mesh = decoder.DecodeMeshFromBuffer(&buffer);
  if (!maybe_mesh.ok()) Fail(std::string("decoding failed: ") + maybe_mesh.status().error_msg());
  const std::unique_ptr<draco::Mesh> mesh = std::move(maybe_mesh).value();
  const uint32_t points = mesh->num_points();

  std::vector<uint8_t> connectivity;
  for (draco::FaceIndex f(0); f < mesh->num_faces(); ++f) {
    for (const draco::PointIndex& corner : mesh->face(f)) {
      if (corner.value() >= points) Fail("a face names a point past the points");
      AppendU32(&connectivity, corner.value());
    }
  }
  Write(dir + "/connectivity.bin", connectivity);
  std::printf("points %u\nfaces %u\n", points, mesh->num_faces());

  for (int i = 0; i < mesh->num_attributes(); ++i) {
    const draco::PointAttribute* a = mesh->attribute(i);
    const int64_t width = int64_t{a->num_components()} * draco::DataTypeLength(a->data_type());
    if (width <= 0) Fail("an attribute of no width");
    std::vector<uint8_t> values;
    values.reserve(static_cast<size_t>(width) * points);
    for (draco::PointIndex p(0); p < points; ++p) {
      const draco::AttributeValueIndex v = a->mapped_index(p);
      if (v.value() >= a->size()) Fail("a point maps past its attribute's values");
      const uint8_t* value = a->GetAddress(v);
      values.insert(values.end(), value, value + width);
    }
    Write(dir + "/attribute-" + std::to_string(a->unique_id()) + ".bin", values);
    std::printf("attribute %d %u %d %d %d %d %" PRId64 " %" PRId64 " %zu %d\n", i, a->unique_id(),
                static_cast<int>(a->attribute_type()), static_cast<int>(a->data_type()),
                a->num_components(), a->normalized() ? 1 : 0, a->byte_stride(), a->byte_offset(),
                a->size(), a->is_mapping_identity() ? 1 : 0);
  }

  const draco::GeometryMetadata* metadata = mesh->GetMetadata();
  if (metadata) {
    std::string name = "-";
    metadata->GetEntryString("name", &name);
    std::printf("metadata %s %zu\n", name.c_str(), metadata->attribute_metadatas().size());
  }
  return 0;
}
