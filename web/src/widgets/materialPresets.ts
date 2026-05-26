export interface MaterialPreset {
  name: string;
  category: string;
  density: number;
  youngs_modulus: number;
  poissons_ratio: number;
  ultimate_tensile: number;
  yield_compressive: number;
  color: [number, number, number];
  roughness: number;
  metalness: number;
}

export const MATERIAL_PRESETS: MaterialPreset[] = [
  // ── 3D Printing Filaments ─────────────────────────────────
  { name: "PLA",               category: "3D Printing", density: 1240,  youngs_modulus: 3.5,  poissons_ratio: 0.36, ultimate_tensile: 50,   yield_compressive: 80,   color: [0.85, 0.85, 0.85], roughness: 0.45, metalness: 0 },
  { name: "ABS",               category: "3D Printing", density: 1040,  youngs_modulus: 2.3,  poissons_ratio: 0.35, ultimate_tensile: 40,   yield_compressive: 65,   color: [0.92, 0.90, 0.84], roughness: 0.50, metalness: 0 },
  { name: "PETG",              category: "3D Printing", density: 1270,  youngs_modulus: 2.2,  poissons_ratio: 0.38, ultimate_tensile: 53,   yield_compressive: 55,   color: [0.82, 0.87, 0.92], roughness: 0.35, metalness: 0 },
  { name: "Nylon (PA)",        category: "3D Printing", density: 1140,  youngs_modulus: 1.7,  poissons_ratio: 0.39, ultimate_tensile: 70,   yield_compressive: 50,   color: [0.88, 0.84, 0.76], roughness: 0.40, metalness: 0 },
  { name: "TPU",               category: "3D Printing", density: 1210,  youngs_modulus: 0.03, poissons_ratio: 0.48, ultimate_tensile: 40,   yield_compressive: 20,   color: [0.90, 0.90, 0.92], roughness: 0.55, metalness: 0 },
  { name: "ASA",               category: "3D Printing", density: 1070,  youngs_modulus: 2.1,  poissons_ratio: 0.35, ultimate_tensile: 45,   yield_compressive: 60,   color: [0.90, 0.88, 0.82], roughness: 0.50, metalness: 0 },
  { name: "PC",                category: "3D Printing", density: 1200,  youngs_modulus: 2.4,  poissons_ratio: 0.37, ultimate_tensile: 63,   yield_compressive: 86,   color: [0.88, 0.90, 0.92], roughness: 0.30, metalness: 0 },
  { name: "PVA",               category: "3D Printing", density: 1230,  youngs_modulus: 3.0,  poissons_ratio: 0.38, ultimate_tensile: 45,   yield_compressive: 60,   color: [0.92, 0.92, 0.88], roughness: 0.50, metalness: 0 },
  { name: "HIPS",              category: "3D Printing", density: 1050,  youngs_modulus: 2.1,  poissons_ratio: 0.35, ultimate_tensile: 32,   yield_compressive: 40,   color: [0.95, 0.95, 0.93], roughness: 0.50, metalness: 0 },
  { name: "CF-PLA",            category: "3D Printing", density: 1290,  youngs_modulus: 5.5,  poissons_ratio: 0.33, ultimate_tensile: 55,   yield_compressive: 90,   color: [0.20, 0.20, 0.22], roughness: 0.60, metalness: 0 },
  { name: "CF-PETG",           category: "3D Printing", density: 1320,  youngs_modulus: 4.5,  poissons_ratio: 0.34, ultimate_tensile: 58,   yield_compressive: 70,   color: [0.22, 0.22, 0.25], roughness: 0.55, metalness: 0 },
  { name: "CF-Nylon",          category: "3D Printing", density: 1180,  youngs_modulus: 6.5,  poissons_ratio: 0.35, ultimate_tensile: 80,   yield_compressive: 75,   color: [0.18, 0.18, 0.20], roughness: 0.55, metalness: 0 },

  // ── Metals ────────────────────────────────────────────────
  { name: "Mild Steel (1020)", category: "Metal",       density: 7900,  youngs_modulus: 190,  poissons_ratio: 0.29, ultimate_tensile: 440,  yield_compressive: 440,  color: [0.55, 0.56, 0.57], roughness: 0.35, metalness: 1.0 },
  { name: "Stainless 304",    category: "Metal",       density: 7800,  youngs_modulus: 200,  poissons_ratio: 0.28, ultimate_tensile: 580,  yield_compressive: 580,  color: [0.65, 0.66, 0.68], roughness: 0.25, metalness: 1.0 },
  { name: "Stainless 316",    category: "Metal",       density: 7900,  youngs_modulus: 200,  poissons_ratio: 0.28, ultimate_tensile: 580,  yield_compressive: 580,  color: [0.63, 0.64, 0.66], roughness: 0.25, metalness: 1.0 },
  { name: "Aluminum 6061-T6", category: "Metal",       density: 2700,  youngs_modulus: 69,   poissons_ratio: 0.33, ultimate_tensile: 310,  yield_compressive: 310,  color: [0.91, 0.92, 0.93], roughness: 0.30, metalness: 1.0 },
  { name: "Aluminum 7075-T6", category: "Metal",       density: 2810,  youngs_modulus: 70,   poissons_ratio: 0.32, ultimate_tensile: 560,  yield_compressive: 560,  color: [0.88, 0.89, 0.90], roughness: 0.30, metalness: 1.0 },
  { name: "Copper (C11000)",   category: "Metal",       density: 8960,  youngs_modulus: 120,  poissons_ratio: 0.34, ultimate_tensile: 220,  yield_compressive: 220,  color: [0.72, 0.45, 0.20], roughness: 0.25, metalness: 1.0 },
  { name: "Brass (C26000)",    category: "Metal",       density: 8200,  youngs_modulus: 110,  poissons_ratio: 0.31, ultimate_tensile: 400,  yield_compressive: 400,  color: [0.78, 0.68, 0.30], roughness: 0.25, metalness: 1.0 },
  { name: "Titanium Ti-6Al-4V",category: "Metal",       density: 4430,  youngs_modulus: 110,  poissons_ratio: 0.32, ultimate_tensile: 1000, yield_compressive: 1000, color: [0.62, 0.62, 0.64], roughness: 0.30, metalness: 1.0 },
  { name: "Cast Iron (Gray)",  category: "Metal",       density: 7500,  youngs_modulus: 180,  poissons_ratio: 0.29, ultimate_tensile: 250,  yield_compressive: 830,  color: [0.38, 0.38, 0.40], roughness: 0.50, metalness: 0.9 },

  // ── Wood (parallel to grain) ──────────────────────────────
  { name: "Pine",              category: "Wood",        density: 520,   youngs_modulus: 10.0, poissons_ratio: 0.30, ultimate_tensile: 35,   yield_compressive: 40,   color: [0.75, 0.60, 0.40], roughness: 0.70, metalness: 0 },
  { name: "Oak",               category: "Wood",        density: 670,   youngs_modulus: 12.3, poissons_ratio: 0.30, ultimate_tensile: 80,   yield_compressive: 50,   color: [0.55, 0.40, 0.22], roughness: 0.65, metalness: 0 },
  { name: "Birch",             category: "Wood",        density: 620,   youngs_modulus: 13.5, poissons_ratio: 0.30, ultimate_tensile: 70,   yield_compressive: 45,   color: [0.82, 0.72, 0.55], roughness: 0.60, metalness: 0 },
  { name: "Maple",             category: "Wood",        density: 630,   youngs_modulus: 11.3, poissons_ratio: 0.30, ultimate_tensile: 75,   yield_compressive: 48,   color: [0.78, 0.65, 0.45], roughness: 0.60, metalness: 0 },
  { name: "Plywood",           category: "Wood",        density: 550,   youngs_modulus: 8.5,  poissons_ratio: 0.30, ultimate_tensile: 30,   yield_compressive: 35,   color: [0.72, 0.58, 0.38], roughness: 0.65, metalness: 0 },
  { name: "MDF",               category: "Wood",        density: 750,   youngs_modulus: 4.0,  poissons_ratio: 0.25, ultimate_tensile: 18,   yield_compressive: 10,   color: [0.68, 0.58, 0.42], roughness: 0.75, metalness: 0 },
  { name: "Bamboo",            category: "Wood",        density: 700,   youngs_modulus: 18.0, poissons_ratio: 0.30, ultimate_tensile: 160,  yield_compressive: 60,   color: [0.80, 0.72, 0.45], roughness: 0.55, metalness: 0 },

  // ── Glass / Ceramics ──────────────────────────────────────
  { name: "Soda-lime Glass",   category: "Glass",       density: 2500,  youngs_modulus: 70,   poissons_ratio: 0.23, ultimate_tensile: 45,   yield_compressive: 1000, color: [0.85, 0.92, 0.90], roughness: 0.05, metalness: 0 },
  { name: "Borosilicate Glass",category: "Glass",       density: 2230,  youngs_modulus: 67,   poissons_ratio: 0.21, ultimate_tensile: 280,  yield_compressive: 2000, color: [0.90, 0.93, 0.95], roughness: 0.05, metalness: 0 },
  { name: "Alumina Ceramic",   category: "Glass",       density: 3800,  youngs_modulus: 310,  poissons_ratio: 0.22, ultimate_tensile: 230,  yield_compressive: 2240, color: [0.95, 0.95, 0.92], roughness: 0.30, metalness: 0 },

  // ── Engineering Plastics ──────────────────────────────────
  { name: "Nylon 6 (PA6)",     category: "Plastic",     density: 1140,  youngs_modulus: 2.7,  poissons_ratio: 0.39, ultimate_tensile: 75,   yield_compressive: 80,   color: [0.88, 0.85, 0.78], roughness: 0.40, metalness: 0 },
  { name: "Nylon 66 (PA66)",   category: "Plastic",     density: 1140,  youngs_modulus: 3.2,  poissons_ratio: 0.39, ultimate_tensile: 84,   yield_compressive: 90,   color: [0.86, 0.83, 0.76], roughness: 0.40, metalness: 0 },
  { name: "PEEK",              category: "Plastic",     density: 1300,  youngs_modulus: 4.0,  poissons_ratio: 0.38, ultimate_tensile: 97,   yield_compressive: 120,  color: [0.55, 0.50, 0.38], roughness: 0.40, metalness: 0 },
  { name: "Delrin (POM)",      category: "Plastic",     density: 1410,  youngs_modulus: 3.0,  poissons_ratio: 0.35, ultimate_tensile: 67,   yield_compressive: 110,  color: [0.92, 0.92, 0.90], roughness: 0.35, metalness: 0 },
  { name: "HDPE",              category: "Plastic",     density: 950,   youngs_modulus: 1.1,  poissons_ratio: 0.42, ultimate_tensile: 30,   yield_compressive: 20,   color: [0.93, 0.93, 0.90], roughness: 0.45, metalness: 0 },
  { name: "PP",                category: "Plastic",     density: 910,   youngs_modulus: 1.5,  poissons_ratio: 0.42, ultimate_tensile: 35,   yield_compressive: 40,   color: [0.90, 0.90, 0.88], roughness: 0.45, metalness: 0 },
  { name: "PMMA (Acrylic)",    category: "Plastic",     density: 1180,  youngs_modulus: 3.0,  poissons_ratio: 0.37, ultimate_tensile: 72,   yield_compressive: 100,  color: [0.92, 0.94, 0.96], roughness: 0.10, metalness: 0 },

  // ── Composites / Rubber ───────────────────────────────────
  { name: "CFRP (quasi-iso)",  category: "Composite",   density: 1600,  youngs_modulus: 70,   poissons_ratio: 0.30, ultimate_tensile: 600,  yield_compressive: 500,  color: [0.12, 0.12, 0.14], roughness: 0.35, metalness: 0 },
  { name: "GFRP (E-glass)",    category: "Composite",   density: 1850,  youngs_modulus: 25,   poissons_ratio: 0.28, ultimate_tensile: 300,  yield_compressive: 250,  color: [0.80, 0.82, 0.70], roughness: 0.40, metalness: 0 },
  { name: "Silicone Rubber",   category: "Composite",   density: 1150,  youngs_modulus: 0.005,poissons_ratio: 0.49, ultimate_tensile: 7,    yield_compressive: 0,    color: [0.85, 0.85, 0.88], roughness: 0.70, metalness: 0 },
  { name: "Natural Rubber",    category: "Composite",   density: 930,   youngs_modulus: 0.003,poissons_ratio: 0.50, ultimate_tensile: 25,   yield_compressive: 0,    color: [0.70, 0.62, 0.45], roughness: 0.75, metalness: 0 },
];

export const PRESET_CATEGORIES = [...new Set(MATERIAL_PRESETS.map((p) => p.category))];

export function applyPreset(
  current: Record<string, unknown>,
  preset: MaterialPreset,
): Record<string, unknown> {
  return {
    ...current,
    color: preset.color,
    roughness: preset.roughness,
    metalness: preset.metalness,
    density: preset.density,
    youngs_modulus: preset.youngs_modulus,
    poissons_ratio: preset.poissons_ratio,
    yield_tensile: 0,
    ultimate_tensile: preset.ultimate_tensile,
    yield_compressive: preset.yield_compressive,
  };
}
