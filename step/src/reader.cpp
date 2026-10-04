// STEP reader for PolyLoupe, on OpenCASCADE's XDE (names, colors, assemblies).
//
// Reads the file, meshes every distinct part once on all cores (assembly instances share their
// part's triangulation), then writes each instance as triangle soups split by face color into
// the byte buffer described in lib.rs.

#include <BRepLib_ToolTriangulatedShape.hxx>
#include <BRepMesh_IncrementalMesh.hxx>
#include <BRep_Builder.hxx>
#include <BRep_Tool.hxx>
#include <IFSelect_ReturnStatus.hxx>
#include <Poly_Triangulation.hxx>
#include <Quantity_Color.hxx>
#include <STEPCAFControl_Reader.hxx>
#include <Standard_Failure.hxx>
#include <TDF_Label.hxx>
#include <TDataStd_Name.hxx>
#include <TDocStd_Document.hxx>
#include <TopExp_Explorer.hxx>
#include <TopoDS.hxx>
#include <TopoDS_Compound.hxx>
#include <TopoDS_Face.hxx>
#include <XCAFDoc_ColorTool.hxx>
#include <XCAFDoc_DocumentTool.hxx>
#include <XCAFDoc_ShapeTool.hxx>

#include <chrono>
#include <cstdint>
#include <cstdio>
#include <cstdlib>
#include <cstring>
#include <map>
#include <string>
#include <unordered_map>
#include <unordered_set>
#include <vector>

namespace {

struct Color {
    float r, g, b;
    bool operator<(const Color& o) const {
        return r < o.r || (r == o.r && (g < o.g || (g == o.g && b < o.b)));
    }
};

struct Instance {
    TDF_Label part;
    gp_Trsf trsf;
    std::string name;
    bool has_color;
    Color color;
};

struct Walker {
    Handle(XCAFDoc_ShapeTool) shapes;
    std::vector<Instance> out;
};

std::string label_name(const TDF_Label& label) {
    Handle(TDataStd_Name) attr;
    if (label.IsNull() || !label.FindAttribute(TDataStd_Name::GetID(), attr)) return {};
    const TCollection_ExtendedString& name = attr->Get();
    std::string utf8(name.LengthOfCString() + 1, '\0');
    char* buf = utf8.data();
    name.ToUTF8CString(buf);
    utf8.resize(std::strlen(utf8.c_str()));
    return utf8;
}

bool label_color(const TDF_Label& label, Color& out) {
    Quantity_Color c;
    if (XCAFDoc_ColorTool::GetColor(label, XCAFDoc_ColorSurf, c) || XCAFDoc_ColorTool::GetColor(label, XCAFDoc_ColorGen, c)) {
        // Quantity_Color keeps linear RGB, like PolyLoupe's materials.
        out = {float(c.Red()), float(c.Green()), float(c.Blue())};
        return true;
    }
    return false;
}

// Generic CAD names ("SOLID", "Open CASCADE STEP translator ...") say nothing about the part.
bool useful_name(const std::string& name) {
    return !name.empty() && name != "SOLID" && name != "COMPOUND" && name.rfind("Open CASCADE", 0) != 0;
}

void walk(Walker& w, const TDF_Label& label, const gp_Trsf& parent, bool has_color, Color color) {
    TDF_Label part = label;
    gp_Trsf trsf = parent;
    Color c;
    if (XCAFDoc_ShapeTool::IsReference(label)) {
        XCAFDoc_ShapeTool::GetReferredShape(label, part);
        trsf = parent * XCAFDoc_ShapeTool::GetLocation(label).Transformation();
        // The instance's own color wins over the part's.
        if (label_color(label, c) || label_color(part, c)) {
            has_color = true;
            color = c;
        }
    } else if (label_color(label, c)) {
        has_color = true;
        color = c;
    }
    if (XCAFDoc_ShapeTool::IsAssembly(part)) {
        NCollection_Sequence<TDF_Label> components;
        XCAFDoc_ShapeTool::GetComponents(part, components);
        for (int i = 1; i <= components.Length(); i++) walk(w, components.Value(i), trsf, has_color, color);
        return;
    }
    std::string name = label_name(part);
    if (!useful_name(name)) name = label_name(label);
    w.out.push_back({part, trsf, useful_name(name) ? name : std::string(), has_color, color});
}

void put_u32(std::vector<uint8_t>& b, uint32_t v) {
    uint8_t bytes[4];
    std::memcpy(bytes, &v, 4);
    b.insert(b.end(), bytes, bytes + 4);
}

void put_f32(std::vector<uint8_t>& b, float v) {
    uint32_t bits;
    std::memcpy(&bits, &v, 4);
    put_u32(b, bits);
}

struct Soup {
    std::vector<float> positions, normals;
    std::vector<uint32_t> indices;
};

std::vector<uint8_t> read(const char* path, double linear, double angular) {
    Handle(TDocStd_Document) doc = new TDocStd_Document("XmlXCAF");
    STEPCAFControl_Reader reader;
    reader.SetColorMode(true);
    reader.SetNameMode(true);
    // POLYLOUPE_STEP_TIMING=1 prints how long each stage takes.
    const bool timing = std::getenv("POLYLOUPE_STEP_TIMING") != nullptr;
    auto t0 = std::chrono::steady_clock::now();
    auto lap = [&](const char* stage) {
        if (!timing) return;
        auto now = std::chrono::steady_clock::now();
        std::fprintf(stderr, "step %s: %.0f ms\n", stage, std::chrono::duration<double, std::milli>(now - t0).count());
        t0 = now;
    };
    if (reader.ReadFile(path) != IFSelect_RetDone) throw std::runtime_error("not a readable STEP file");
    lap("parse");
    if (!reader.Transfer(doc)) throw std::runtime_error("the STEP file has no shapes OpenCASCADE can use");
    lap("transfer");

    Walker w;
    w.shapes = XCAFDoc_DocumentTool::ShapeTool(doc->Main());
    NCollection_Sequence<TDF_Label> roots;
    w.shapes->GetFreeShapes(roots);
    for (int i = 1; i <= roots.Length(); i++) walk(w, roots.Value(i), gp_Trsf(), false, Color{});

    // Distinct parts, meshed together on all cores.
    std::unordered_map<TDF_Label, TopoDS_Shape> part_shapes;
    BRep_Builder builder;
    TopoDS_Compound all;
    builder.MakeCompound(all);
    for (const Instance& inst : w.out) {
        if (part_shapes.count(inst.part)) continue;
        TopoDS_Shape shape = XCAFDoc_ShapeTool::GetShape(inst.part);
        part_shapes.emplace(inst.part, shape);
        if (!shape.IsNull()) builder.Add(all, shape);
    }
    BRepMesh_IncrementalMesh mesher(all, linear, true, angular, true);
    lap("mesh");

    // Face colors set on a part's sub-shapes, by face.
    std::unordered_map<const void*, Color> face_colors;
    for (auto& [label, shape] : part_shapes) {
        NCollection_Sequence<TDF_Label> subs;
        XCAFDoc_ShapeTool::GetSubShapes(label, subs);
        for (int i = 1; i <= subs.Length(); i++) {
            Color c;
            if (!label_color(subs.Value(i), c)) continue;
            TopoDS_Shape sub = XCAFDoc_ShapeTool::GetShape(subs.Value(i));
            for (TopExp_Explorer ex(sub, TopAbs_FACE); ex.More(); ex.Next()) face_colors[ex.Current().TShape().get()] = c;
        }
    }

    const Color grey{0.8f, 0.8f, 0.8f};
    std::vector<uint8_t> out;
    put_u32(out, 0); // part count, patched at the end
    uint32_t parts = 0;
    std::unordered_set<const void*> normals_done;
    // One object per solid: a part that is a bare compound of solids (no assembly structure)
    // would otherwise come out as a single object. Faces outside any solid stay together.
    auto pieces_of = [](const TopoDS_Shape& shape) {
        std::vector<TopoDS_Shape> solids;
        for (TopExp_Explorer ex(shape, TopAbs_SOLID); ex.More(); ex.Next()) solids.push_back(ex.Current());
        if (solids.size() <= 1) return std::vector<TopoDS_Shape>{shape};
        BRep_Builder b;
        TopoDS_Compound loose;
        b.MakeCompound(loose);
        bool any_loose = false;
        for (TopExp_Explorer ex(shape, TopAbs_FACE, TopAbs_SOLID); ex.More(); ex.Next()) {
            b.Add(loose, ex.Current());
            any_loose = true;
        }
        if (any_loose) solids.push_back(loose);
        return solids;
    };
    for (const Instance& inst : w.out) {
        const TopoDS_Shape& shape = part_shapes[inst.part];
        if (shape.IsNull()) continue;
        const bool mirrored = inst.trsf.IsNegative();
        const std::vector<TopoDS_Shape> pieces = pieces_of(shape);
        for (size_t k = 0; k < pieces.size(); k++) {
            std::string name = inst.name;
            if (pieces.size() > 1 && !name.empty()) name += " " + std::to_string(k + 1);
            std::map<Color, Soup> by_color;
            for (TopExp_Explorer ex(pieces[k], TopAbs_FACE); ex.More(); ex.Next()) {
                const TopoDS_Face& face = TopoDS::Face(ex.Current());
                TopLoc_Location loc;
                Handle(Poly_Triangulation) tri = BRep_Tool::Triangulation(face, loc);
                if (tri.IsNull() || tri->NbTriangles() == 0) continue;
                // Normals from the exact surface; shared by every instance of the part.
                if (normals_done.insert(tri.get()).second && !tri->HasNormals()) BRepLib_ToolTriangulatedShape::ComputeNormals(face, tri);
                auto fc = face_colors.find(face.TShape().get());
                Color color = fc != face_colors.end() ? fc->second : inst.has_color ? inst.color : grey;
                Soup& soup = by_color[color];
                const gp_Trsf t = inst.trsf * loc.Transformation();
                const bool reversed = face.Orientation() == TopAbs_REVERSED;
                const bool flip = reversed != mirrored;
                const uint32_t base = uint32_t(soup.positions.size() / 3);
                for (int i = 1; i <= tri->NbNodes(); i++) {
                    gp_Pnt p = tri->Node(i).Transformed(t);
                    soup.positions.insert(soup.positions.end(), {float(p.X()), float(p.Y()), float(p.Z())});
                    gp_Dir n = tri->HasNormals() ? tri->Normal(i) : gp_Dir(0, 0, 1);
                    n.Transform(t);
                    if (reversed) n.Reverse();
                    soup.normals.insert(soup.normals.end(), {float(n.X()), float(n.Y()), float(n.Z())});
                }
                for (int i = 1; i <= tri->NbTriangles(); i++) {
                    int a, b, c;
                    tri->Triangle(i).Get(a, b, c);
                    if (flip) std::swap(b, c);
                    soup.indices.insert(soup.indices.end(), {base + a - 1, base + b - 1, base + c - 1});
                }
            }
            for (auto& [color, soup] : by_color) {
                put_u32(out, uint32_t(name.size()));
                out.insert(out.end(), name.begin(), name.end());
                for (float v : {color.r, color.g, color.b, 1.0f}) put_f32(out, v);
                put_u32(out, uint32_t(soup.positions.size() / 3));
                for (float v : soup.positions) put_f32(out, v);
                for (float v : soup.normals) put_f32(out, v);
                put_u32(out, uint32_t(soup.indices.size()));
                for (uint32_t v : soup.indices) put_u32(out, v);
                parts++;
            }
        }
    }
    lap("extract");
    if (parts == 0) throw std::runtime_error("no shape in the file could be meshed");
    std::memcpy(out.data(), &parts, 4);
    return out;
}

} // namespace

// Returns 0 and the mesh buffer, or non-zero and a UTF-8 error message. Free with
// pl_step_free.
extern "C" int pl_step_read(const char* path, double linear, double angular, uint8_t** out, size_t* out_len) {
    std::vector<uint8_t> bytes;
    int status = 0;
    try {
        bytes = read(path, linear, angular);
    } catch (const Standard_Failure& e) {
        std::string msg = std::string("OpenCASCADE: ") + (e.GetMessageString() ? e.GetMessageString() : "error");
        bytes.assign(msg.begin(), msg.end());
        status = 1;
    } catch (const std::exception& e) {
        std::string msg = e.what();
        bytes.assign(msg.begin(), msg.end());
        status = 1;
    } catch (...) {
        std::string msg = "OpenCASCADE failed while reading the file";
        bytes.assign(msg.begin(), msg.end());
        status = 1;
    }
    *out_len = bytes.size();
    *out = static_cast<uint8_t*>(std::malloc(bytes.size() ? bytes.size() : 1));
    if (!bytes.empty()) std::memcpy(*out, bytes.data(), bytes.size());
    return status;
}

extern "C" void pl_step_free(uint8_t* p) {
    std::free(p);
}
