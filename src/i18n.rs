//! UI translations.
//!
//! Strings are written in English in the code and looked up with [`tr`] (plain text) or [`trf`]
//! (text with `{name}` placeholders). English is the key, so a missing translation simply shows
//! the English text. Technical terms artists know from Blender (Wireframe, Solid, MatCap,
//! X-Ray, Base Color...) stay in English on purpose.

use std::sync::atomic::{AtomicBool, Ordering};

use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Language {
    /// Follow the Windows display language.
    System,
    English,
    Italian,
}

impl Language {
    pub const ALL: [Language; 3] = [Language::System, Language::English, Language::Italian];

    /// Each language is named in itself, so it can be found whatever the current one is.
    pub fn label(self) -> String {
        match self {
            Language::System => {
                let name = if system_is_italian() { "Italiano" } else { "English" };
                format!("{} ({name})", tr("System"))
            }
            Language::English => "English".into(),
            Language::Italian => "Italiano".into(),
        }
    }
}

static ITALIAN: AtomicBool = AtomicBool::new(false);

pub fn set(language: Language) {
    let italian = match language {
        Language::System => system_is_italian(),
        Language::English => false,
        Language::Italian => true,
    };
    ITALIAN.store(italian, Ordering::Relaxed);
}

fn system_is_italian() -> bool {
    sys_locale::get_locale().is_some_and(|l| l.to_ascii_lowercase().starts_with("it"))
}

pub fn italian() -> bool {
    ITALIAN.load(Ordering::Relaxed)
}

/// Integer with the language's thousands separator (1,234 / 1.234).
pub fn thousands(n: usize) -> String {
    let sep = if italian() { '.' } else { ',' };
    let s = n.to_string();
    let mut out = String::with_capacity(s.len() + s.len() / 3);
    for (i, c) in s.chars().enumerate() {
        if i > 0 && (s.len() - i) % 3 == 0 {
            out.push(sep);
        }
        out.push(c);
    }
    out
}

/// Decimal number text with the language's decimal separator.
pub fn decimal(text: String) -> String {
    if italian() { text.replace('.', ",") } else { text }
}

/// Translates `en`; anything without a translation (file names, numbers) passes through.
pub fn tr(en: &str) -> &str {
    if ITALIAN.load(Ordering::Relaxed) { italian_text(en).unwrap_or(en) } else { en }
}

/// Translates `template`, then fills `{name}` placeholders.
pub fn trf(template: &str, args: &[(&str, &dyn std::fmt::Display)]) -> String {
    let mut out = tr(template).to_string();
    for (name, value) in args {
        out = out.replace(&format!("{{{name}}}"), &value.to_string());
    }
    out
}

fn italian_text(en: &str) -> Option<&'static str> {
    Some(match en {
        // Blender terms, kept in English on purpose.
        "V-Sync" => "V-Sync",
        "Wireframe" => "Wireframe",
        "X-Ray" => "X-Ray",
        "Alpha" => "Alpha",
        "Backface culling" => "Backface culling",
        "Outliner" => "Outliner",

        // Menus
        "File" => "File",
        "Edit" => "Modifica",
        "View" => "Vista",
        "Select" => "Seleziona",
        "Help" => "Aiuto",
        "Open…" => "Apri…",
        "Open Recent" => "Apri recenti",
        "Clear Recent Files" => "Cancella file recenti",
        "Load HDRI Environment…" => "Carica ambiente HDRI…",
        "Export Image…" => "Esporta immagine…",
        "Quit" => "Esci",
        "Preferences…" => "Preferenze…",
        "Sidebar" => "Barra laterale",
        "Frame All" => "Inquadra tutto",
        "Frame Selected" => "Inquadra selezione",
        "Perspective" => "Prospettiva",
        "Orthographic" => "Ortografica",
        "Auto Perspective" => "Prospettiva automatica",
        "Toggle Fullscreen" => "Schermo intero",
        "All" => "Tutto",
        "None" => "Niente",
        "Hide Selected" => "Nascondi selezionati",
        "Hide Unselected" => "Nascondi non selezionati",
        "Reveal Hidden" => "Mostra nascosti",

        // Help
        "Navigation" => "Navigazione",
        "Orbit" => "Orbita",
        "Pan" => "Sposta",
        "Zoom" => "Zoom",
        "Frame all" => "Inquadra tutto",
        "Frame selected" => "Inquadra selezione",
        "Front / Right / Top (opposite)" => "Fronte / destra / alto (opposto)",
        "Orbit in steps" => "Orbita a scatti",
        "Perspective / Orthographic" => "Prospettiva / ortografica",
        "Selection" => "Selezione",
        "Select / extend" => "Seleziona / estendi",
        "Select all / none" => "Seleziona tutto / niente",
        "Hide / hide others / reveal" => "Nascondi / nascondi altri / mostra",
        "Sidebar with outliner" => "Barra laterale con outliner",
        "Animation" => "Animazione",
        "Play / pause" => "Riproduci / pausa",
        "Previous / next frame" => "Frame precedente / successivo",
        "Jump to start / end" => "Vai all'inizio / alla fine",
        "Shading" => "Shading",
        "Shading pie menu" => "Menu a torta dello shading",
        "Toggle wireframe" => "Attiva/disattiva wireframe",
        "Toggle X-ray" => "Attiva/disattiva X-ray",
        "Toggle overlays" => "Attiva/disattiva overlay",
        "Export image" => "Esporta immagine",
        "Wheel" => "Rotella",
        "Space" => "Spazio",

        // Status bar, overlays, empty state
        "Preparing lighting…" => "Preparo l'illuminazione…",
        "{tris} tris · loaded in {ms} ms" => "{tris} tri · caricato in {ms} ms",
        "Drop a 3D file here" => "Trascina qui un file 3D",
        "Open File…" => "Apri file…",
        "Recent" => "Recenti",
        "Dismiss" => "Chiudi",
        "Opening {name}…" => "Apertura di {name}…",
        "Drop to open" => "Rilascia per aprire",
        "Objects" => "Oggetti",
        "Vertices" => "Vertici",
        "Triangles" => "Triangoli",
        "Size" => "Dimensioni",
        "Grid" => "Griglia",
        "{view} {projection}" => "{projection} {view}",
        "User" => "utente",
        "Front" => "Frontale",
        "Back" => "Posteriore",
        "Right" => "Destra",
        "Left" => "Sinistra",
        "Top" => "Superiore",
        "Bottom" => "Inferiore",

        // Files and messages
        "Open 3D file" => "Apri file 3D",
        "3D models" => "Modelli 3D",
        "Load HDRI environment" => "Carica ambiente HDRI",
        "This file" => "Questo file",
        "{ext} isn't supported yet. Supported: glTF, GLB, FBX, OBJ, STL, PLY, 3MF, DAE, STEP, and .hdr / .exr environments." => {
            "{ext} non è ancora supportato. Supportati: glTF, GLB, FBX, OBJ, STL, PLY, 3MF, DAE, STEP e ambienti .hdr / .exr."
        }
        "Couldn't open {name}: {error}" => "Impossibile aprire {name}: {error}",
        "the model needs {needed} MB GPU buffers, this GPU allows {limit} MB" => {
            "il modello richiede buffer GPU da {needed} MB, questa GPU ne consente {limit} MB"
        }
        "{first} (+{more} more)" => "{first} (+{more} altri)",
        "Missing texture: {name}" => "Texture mancante: {name}",
        "Unsupported glTF extensions ({list}): the model may look wrong or be incomplete" => {
            "Estensioni glTF non supportate ({list}): il modello potrebbe apparire sbagliato o incompleto"
        }
        "Couldn't decode texture {name}: {error}" => "Impossibile decodificare la texture {name}: {error}",
        "Save image" => "Salva immagine",
        "PNG image" => "Immagine PNG",
        "Saved {name}" => "Salvata {name}",
        "Model saved: {name}" => "Modello salvato: {name}",
        "Manufacturing" => "Manifattura",
        "3D Art" => "3D Art",
        "Workspace\nManufacturing: CAD and 3D printing, millimeters, plain plastic material\n3D Art: textures, UVs, animation, the file's own materials" => {
            "Area di lavoro\nManifattura: CAD e stampa 3D, millimetri, materiale plastica neutra\n3D Art: texture, UV, animazione, i materiali del file"
        }
        "PolyLoupe {version} is available" => "È disponibile PolyLoupe {version}",
        "Surface imperfection" => "Imperfezioni superficie",
        "Fine relief and uneven gloss over the whole model, for a more realistic preview. 0 = perfectly smooth" => {
            "Rilievo fine e lucentezza non uniforme su tutto il modello, per un'anteprima più realistica. 0 = perfettamente liscio"
        }
        "Download" => "Scarica",
        "Check for updates" => "Controlla gli aggiornamenti",
        "Once a day, PolyLoupe asks GitHub whether a newer release exists. Nothing is downloaded or installed" => {
            "Una volta al giorno PolyLoupe chiede a GitHub se esiste una versione più recente. Non scarica né installa nulla"
        }
        "Hide the side panel (N)" => "Nascondi il pannello laterale (N)",
        "Show the side panel (N)" => "Mostra il pannello laterale (N)",
        "Part Material" => "Materiale del pezzo",
        "Plastic" => "Plastica",
        "File materials" => "Materiali del file",
        "The colors and materials stored in the file" => "I colori e i materiali salvati nel file",
        "Satin plastic, the file's materials are ignored" => "Plastica satinata, i materiali del file sono ignorati",
        "Smooth satin plastic, no layer lines" => "Plastica satinata liscia, senza layer",
        "Not stored (mm)" => "Non salvate (mm)",
        "Print Finish" => "Finitura di stampa",
        "Resin" => "Resina",
        "Metal SLM" => "Metallo SLM",
        "FDM, matte, visible layer lines" => "FDM, opaco, layer visibili",
        "FDM, glossy, visible layer lines" => "FDM, lucido, layer visibili",
        "FDM, satin metallic sheen" => "FDM, riflesso satinato metallico",
        "SLA/MSLA, smooth with fine layers" => "SLA/MSLA, liscio con layer sottili",
        "Powder bed, grainy and matte" => "Letto di polvere, granuloso e opaco",
        "Laser-sintered metal, grainy" => "Metallo sinterizzato laser, granuloso",
        "Layer height" => "Altezza layer",
        "Layer lines run along Z: lay the part on a face to change the print direction" => {
            "I layer seguono l'asse Z: appoggia il pezzo su una faccia per cambiare la direzione di stampa"
        }
        "Click the face that goes on the bed · Esc to cancel" => "Clicca la faccia da appoggiare sul piano · Esc per annullare",
        "This face goes on the bed" => "Questa faccia va sul piano",
        "Pick the workspace from the file type" => "Scegli l'area di lavoro dal tipo di file",
        "STL, 3MF, STEP and PLY open in Manufacturing, the other formats in 3D Art. Off: the workspace you chose last" => {
            "STL, 3MF, STEP e PLY si aprono in Manifattura, gli altri formati in 3D Art. Disattivata: l'ultima area scelta"
        }
        "Couldn't save {name}: {error}" => "Impossibile salvare {name}: {error}",
        "Export Model…" => "Esporta modello…",
        "Export Model" => "Esporta modello",
        "Save the model as STL or 3MF, with the Move tool's changes" => {
            "Salva il modello come STL o 3MF, con le modifiche dello strumento Sposta"
        }
        "Save the moved model as STL or 3MF" => "Salva il modello spostato come STL o 3MF",
        "Still analyzing the model, try again in a moment" => "Analisi del modello in corso, riprova tra un attimo",
        "No flat face to lay the model on" => "Nessuna faccia piana su cui appoggiare il modello",
        "Select an object" => "Seleziona un oggetto",
        "Click a face to put it down on the bed (L)" => "Clicca una faccia per appoggiarla sul piano (L)",
        "Lay the model on its largest flat side (Shift L)" => "Appoggia il modello sul lato piano più grande (Shift L)",
        "Move down until it touches the bed (B)" => "Abbassa finché tocca il piano (B)",
        "Back to the position in the file (Alt G)" => "Torna alla posizione del file (Alt G)",
        "Tools" => "Strumenti",
        "Background" => "Sfondo",
        "Studio" => "Studio",
        "World" => "Ambiente",
        "Light gray sweep without the grid, for product shots" => "Fondale grigio chiaro senza griglia, per foto di prodotto",
        "Light and Shadows" => "Luci e ombre",
        "Key light shadows" => "Ombre della luce principale",
        "A light from the environment's brightest spot, casting shadows; it turns with the environment" => {
            "Una luce dal punto più luminoso dell'ambiente, che proietta ombre; ruota con l'ambiente"
        }
        "Light strength" => "Intensità luce",
        "Shadow on the floor" => "Ombra sul pavimento",
        "A floor under the model that catches its shadow and contact shading" => {
            "Un pavimento sotto il modello che riceve la sua ombra e l'ombra di contatto"
        }
        "Softness" => "Morbidezza",
        "One color" => "Colore unico",
        "FDM plastic" => "Plastica FDM",
        "Metal" => "Metallo",
        "PLA Silk" => "PLA Silk",
        "Matte, visible layer lines" => "Opaco, layer visibili",
        "Satin metallic sheen, layer lines" => "Riflesso metallico satinato, layer visibili",
        "Glossy, visible layer lines" => "Lucido, layer visibili",
        "Satin, visible layer lines" => "Satinato, layer visibili",
        "Machined or sintered metal" => "Metallo lavorato o sinterizzato",
        "Polished" => "Lucidato",
        "Satin" => "Satinato",
        "Brushed" => "Spazzolato",
        "Blasted" => "Sabbiato",
        "Layer lines" => "Linee dei layer",
        "Rendered mode shows the material; Solid only its color and relief." => {
            "Il materiale si vede in Renderizzato; in Solido solo colore e rilievo."
        }
        "Surface Wear" => "Usura superficie",
        "Grain" => "Grana",
        "Scratches" => "Graffi",
        "Pattern size" => "Scala motivo",
        "Fine relief, uneven gloss and dust, so the part looks less like a perfect CG surface" => {
            "Rilievo fine, lucentezza irregolare e polvere, perché il pezzo non sembri una superficie CG perfetta"
        }
        "Handling marks: on metal they catch the light, on plastic they whiten" => {
            "Segni d'uso: sul metallo catturano la luce, sulla plastica sbiancano"
        }
        "Scale of the grain and scratch patterns" => "Scala dei motivi di grana e graffi",
        "Hex color, like #D9D9D6" => "Colore esadecimale, per esempio #D9D9D6",
        "Move (W)" => "Sposta (W)",
        "Rotate (E)" => "Ruota (E)",
        "Scale (R)" => "Scala (R)",
        "Lay on face" => "Appoggia su faccia",
        "Auto orient" => "Orienta in automatico",
        "Drop to bed" => "Porta sul piano",
        "Reset" => "Ripristina",
        "Face to lay on the bed" => "Faccia da appoggiare sul piano",
        "Drag a ring" => "Trascina un anello",
        "Drag a handle, or the center for all axes" => "Trascina una maniglia, o il centro per tutti gli assi",
        "Drag an arrow, or the center" => "Trascina una freccia, o il centro",
        "Snap" => "Scatti",
        "Undo" => "Annulla",
        "Cancel" => "Annulla",
        "Couldn't save the image: {error}" => "Impossibile salvare l'immagine: {error}",
        "Couldn't render the image" => "Impossibile renderizzare l'immagine",

        // Header
        "Channel" => "Canale",
        "All objects" => "Tutti gli oggetti",
        "1 selected object" => "1 oggetto selezionato",
        "{n} selected objects" => "{n} oggetti selezionati",
        "Show overlays (Shift Alt Z)" => "Mostra overlay (Shift Alt Z)",
        "Overlay options" => "Opzioni overlay",
        "Toggle X-ray (Alt Z)" => "Attiva/disattiva X-ray (Alt Z)",
        "{mode}  ·  Z for the pie menu" => "{mode}  ·  Z per il menu a torta",
        "Shading settings: they change with the mode (panel on the right)" => {
            "Impostazioni shading: cambiano con la modalità (pannello a destra)"
        }
        "Mode" => "Modalità",
        "The options below change with the mode." => "Le opzioni qui sotto cambiano con la modalità.",
        "Sidebar with outliner (N)" => "Barra laterale con outliner (N)",

        // Sidebar
        "Open a file to see its objects." => "Apri un file per vederne gli oggetti.",
        "{n} objects" => "{n} oggetti",
        "Object" => "Oggetto",
        "Dimensions" => "Dimensioni",
        "Material" => "Materiale",
        "Texture Maps" => "Mappe texture",
        "Hide (H)" => "Nascondi (H)",
        "Show (Alt H)" => "Mostra (Alt H)",
        "{name}\nClick to select · Shift/Ctrl to extend · Double-click to frame" => {
            "{name}\nClic per selezionare · Shift/Ctrl per estendere · doppio clic per inquadrare"
        }
        "{name}\n{w} × {h}\nClick to show this channel on the object (again to turn it off)" => {
            "{name}\n{w} × {h}\nClic per mostrare questo canale sull'oggetto (di nuovo per spegnerlo)"
        }

        // Popovers
        "Viewport Overlays" => "Overlay della vista",
        "Floor grid" => "Griglia a pavimento",
        "Axes" => "Assi",
        "Statistics" => "Statistiche",
        "Navigation gizmo" => "Gizmo di navigazione",
        "Performance" => "Prestazioni",
        "Graphics" => "Grafica",
        "Quality" => "Qualità",
        "Performance: for integrated GPUs and older PCs. No anti-aliasing, the view at 100% scale on high-DPI screens, lighter shadows. Exported images keep full quality" => "Prestazioni: per GPU integrate e PC datati. Niente anti-aliasing, vista al 100% sugli schermi ad alta densità, ombre più leggere. Le immagini esportate restano alla massima qualità",
        "Off: frames aren't capped to the monitor refresh rate" => {
            "Spento: i frame non sono limitati alla frequenza del monitor"
        }
        "Frame rate" => "Frame rate",
        "Redraws continuously and shows FPS in the status bar" => {
            "Ridisegna di continuo e mostra gli FPS nella barra di stato"
        }
        "Wireframe Color" => "Colore wireframe",
        "Theme" => "Tema",
        "Random" => "Casuale",
        "Options" => "Opzioni",
        "Object outlines" => "Contorni oggetti",
        "Measure (M)" => "Misura (M)",
        "First point" => "Primo punto",
        "Second point" => "Secondo punto",
        "Cancel point" => "Annulla punto",
        "Remove last" => "Rimuovi ultima",
        "Clear all" => "Cancella tutte",
        "Back to select" => "Torna a Seleziona",
        "No vertex snap" => "Senza snap ai vertici",
        "Section" => "Sezione",
        "Compare with…" => "Confronta con…",
        "Compare with another model (Ctrl Shift O), or drop it on the right half" => {
            "Confronta con un altro modello (Ctrl Shift O), o trascinalo sulla metà destra"
        }
        "A/B side by side" => "A/B affiancati",
        "A/B split: drag the divider" => "A/B a tendina: trascina il divisore",
        "Swap A and B" => "Scambia A e B",
        "Close the comparison" => "Chiudi il confronto",
        "Comparison" => "Confronto",
        "Up axis" => "Asse verticale",
        "Z up" => "Z in alto",
        "Y up" => "Y in alto",
        "Z: Blender, 3ds Max, Unreal. Y: Maya, Unity, Houdini, ZBrush, Substance. Only names, colors and numbers change; the model looks the same." => {
            "Z: Blender, 3ds Max, Unreal. Y: Maya, Unity, Houdini, ZBrush, Substance. Cambiano solo nomi, colori e numeri; il modello resta uguale."
        }
        "Y, height" => "Y, altezza",
        "Z, depth" => "Z, profondità",
        "Export Turntable" => "Esporta turntable",
        "Export Turntable…" => "Esporta turntable…",
        "Format" => "Formato",
        "MP4 needs ffmpeg on the PATH (winget install ffmpeg). GIF works without it." => {
            "L'MP4 richiede ffmpeg nel PATH (winget install ffmpeg). La GIF funziona senza."
        }
        "Long side in pixels; the shape follows the view" => "Lato lungo in pixel; le proporzioni seguono la vista",
        "Play the animation during the turn" => "Riproduci l'animazione durante il giro",
        "Turns once around the model from the current view, with the current shading." => {
            "Un giro completo attorno al modello dalla vista attuale, con lo shading attuale."
        }
        "Export…" => "Esporta…",
        "MP4 video" => "Video MP4",
        "GIF animation" => "Animazione GIF",
        "ffmpeg stopped while encoding" => "ffmpeg si è fermato durante la codifica",
        "Encoding the turntable…" => "Codifica del turntable in corso…",
        "Couldn't start ffmpeg: {error}" => "Impossibile avviare ffmpeg: {error}",
        "ffmpeg couldn't encode the video" => "ffmpeg non è riuscito a codificare il video",
        "Triangle budget" => "Budget triangoli",
        "Drag or type your target; 0 turns it off" => "Trascina o scrivi il tuo obiettivo; 0 lo disattiva",
        "No texture: assuming 2048 px" => "Nessuna texture: ipotizzo 2048 px",
        "No UVs" => "Nessuna UV",
        "Objects without a texture assume 2048 px" => "Gli oggetti senza texture ipotizzano 2048 px",
        "Texel density" => "Densità texel",
        "Origins" => "Origini",
        "Each object's pivot as a dot, like Blender" => "Il pivot di ogni oggetto come un punto, come in Blender",
        "Pivot" => "Pivot",
        "Report a bug or suggest a feature…" => "Segnala un bug o proponi una funzione…",
        "Check for updates…" => "Controlla aggiornamenti…",
        "Show pivots in the viewport, with the active object's axes" => "Mostra i pivot nella vista, con gli assi dell'oggetto attivo",
        "Unknown" => "Sconosciuto",
        "Outside the model" => "Fuori dal modello",
        "Bottom center" => "Base, centrato",
        "Center" => "Centro",
        "Top center" => "Cima, centrato",
        "Off center" => "Decentrato",
        "Meters" => "Metri",
        "Centimeters" => "Centimetri",
        "Millimeters" => "Millimetri",
        "Inches" => "Pollici",
        "Feet" => "Piedi",
        "Kilometers" => "Chilometri",
        "Meters (glTF)" => "Metri (glTF)",
        "Not stored (m)" => "Non salvate (m)",
        "On the floor" => "Sul pavimento",
        "Raised +{d}" => "Sollevato +{d}",
        "Sunk −{d}" => "Sotto −{d}",
        "Scale" => "Scala",
        "File units" => "Unità del file",
        "World origin" => "Origine del mondo",
        "Ground" => "Appoggio",
        "{size} across: huge for a single asset. If it was modeled in centimeters and read as meters, it's 100× too big ({real})." => {
            "{size} di lato: enorme per un asset singolo. Se era modellato in centimetri e letto come metri, è 100× troppo grande ({real})."
        }
        "{size} across: tiny. If it was modeled in meters and exported as millimeters, it's 1000× too small ({real})." => {
            "{size} di lato: minuscolo. Se era modellato in metri ed esportato come millimetri, è 1000× troppo piccolo ({real})."
        }
        "This format doesn't store units. If the author worked in millimeters (common for STL), the real size is {real}." => {
            "Questo formato non salva le unità. Se l'autore lavorava in millimetri (comune per gli STL), la dimensione reale è {real}."
        }
        "Normals" => "Normali",
        "A line along each vertex normal" => "Una linea lungo la normale di ogni vertice",
        "Length" => "Lunghezza",
        "Face orientation" => "Orientamento facce",
        "Front faces blue, back faces red: flipped faces show up red" => {
            "Fronte delle facce in blu, retro in rosso: le facce girate appaiono rosse"
        }
        "Inverted normals" => "Normali invertite",
        "UV layout (U)" => "Layout UV (U)",
        "No UVs on these objects" => "Questi oggetti non hanno UV",
        "Mirrored" => "Ribaltate",
        "Outside 0–1" => "Fuori 0–1",
        "Close UV view (U)" => "Chiudi la vista UV (U)",
        "UVs of the selected objects" => "UV degli oggetti selezionati",
        "Texture set: every object using one material" => "Texture set: tutti gli oggetti con un materiale",
        "Fit the 0–1 square (double-click)" => "Inquadra il quadrato 0–1 (doppio clic)",
        "Base color texture behind the layout" => "Texture base color dietro il layout",
        "Texture" => "Texture",
        "Non-manifold" => "Non-manifold",
        "Overlapping" => "Sovrapposti",
        "Size {axis}" => "Dimensione {axis}",
        "Textures" => "Texture",
        "Drop to compare" => "Rilascia per confrontare",
        "Replaces the model" => "Sostituisce il modello",
        "Opens it as B, next to A" => "Lo apre come B, accanto ad A",
        "Drag the plane" => "Trascina il piano",
        "Cut across this axis" => "Taglia lungo questo asse",
        "Drag in the view with the Section tool to move it" => "Trascina nella vista con lo strumento Sezione per spostarlo",
        "Flip" => "Inverti",
        "Keep the other side" => "Tieni l'altro lato",
        "Turn the section off" => "Disattiva la sezione",
        "Del" => "Canc",
        "Dark line around each object in Solid mode" => "Linea scura attorno a ogni oggetto in modalità Solido",
        "Mesh Analysis" => "Analisi mesh",
        "Mesh Check" => "Controllo mesh",
        "Analyzing…" => "Analisi in corso…",
        "Non-manifold edges" => "Spigoli non-manifold",
        "Open edges" => "Spigoli aperti",
        "Overlapping vertices" => "Vertici sovrapposti",
        "Degenerate faces" => "Facce degeneri",
        "Edges shared by more than two faces, or between faces with flipped normals" => {
            "Spigoli condivisi da più di due facce, o tra facce con normali invertite"
        }
        "Edges with a single face: holes and open borders" => "Spigoli con una sola faccia: buchi e bordi aperti",
        "Separate vertices closer than 0.1 mm, what Merge by Distance would weld" => {
            "Vertici separati a meno di 0,1 mm, quelli che Merge by Distance salderebbe"
        }
        "Click to show them in the viewport" => "Clicca per vederli nella vista",
        "No problems found: the mesh is closed and clean." => "Nessun problema: la mesh è chiusa e pulita.",
        "Lighting" => "Illuminazione",
        "Flat" => "Piatta",
        "Color" => "Colore",
        "Single" => "Singolo",
        "Attribute" => "Attributo",
        "Texture: image maps and their passes · Attribute: vertex colors" => {
            "Texture: mappe immagine e loro canali · Attributo: colori dei vertici"
        }
        "Object color" => "Colore oggetto",
        "Pass" => "Canale",
        "Lit with the current lighting" => "Illuminato con la luce corrente",
        "Raw values, unlit (like Blender's Non-Color)" => "Valori grezzi, senza luce (come Non-Color di Blender)",
        "Environment" => "Ambiente",
        "Forest" => "Foresta",
        "Sunset" => "Tramonto",
        "Load HDRI…" => "Carica HDRI…",
        "or drop a .hdr / .exr" => "o trascina un .hdr / .exr",
        "Right-click for options" => "Clic destro per le opzioni",
        "Use as default" => "Usa come predefinito",
        "Remove from list" => "Rimuovi dall'elenco",
        "Default environment" => "Ambiente predefinito",
        "Rotation" => "Rotazione",
        "Strength" => "Intensità",
        "World background" => "Sfondo del mondo",
        "Blur" => "Sfocatura",
        "Color Management" => "Gestione colore",
        "Exposure" => "Esposizione",
        "Applies to: {target}" => "Si applica a: {target}",
        "Click again for material colors" => "Clicca di nuovo per i colori materiale",
        "C / Shift C: next / previous channel" => "C / Shift C: canale successivo / precedente",
        "Reset ({n})" => "Azzera ({n})",
        "Remove channel overrides from every object" => "Rimuovi i canali impostati su ogni oggetto",

        // Timeline
        "Jump to start (Shift ←)" => "Vai all'inizio (Shift ←)",
        "Previous frame (←)" => "Frame precedente (←)",
        "Pause (Space)" => "Pausa (Spazio)",
        "Play (Space)" => "Riproduci (Spazio)",
        "Next frame (→)" => "Frame successivo (→)",
        "Jump to end (Shift →)" => "Vai alla fine (Shift →)",
        "Animation clip" => "Clip di animazione",
        "{seconds} s at {fps} fps" => "{seconds} s a {fps} fps",
        "Playback speed" => "Velocità di riproduzione",
        "Loop" => "Ripeti",

        // Pie menu and gizmo
        "Toggle X-Ray" => "Attiva/disattiva X-Ray",
        "{view} view" => "Vista {view}",
        "Drag to orbit · click an axis to align the view" => {
            "Trascina per orbitare · clic su un asse per allineare la vista"
        }
        "Zoom · drag up/down" => "Zoom · trascina su/giù",
        "Pan · drag" => "Sposta · trascina",
        "Switch to perspective (Numpad 5)" => "Passa in prospettiva (Numpad 5)",
        "Switch to orthographic (Numpad 5)" => "Passa in ortografica (Numpad 5)",

        // PolyLoupe shell: title bar, tools, inspector, footer, empty state
        "Inspector" => "Pannello",
        "Inspector (N)" => "Pannello (N)",
        "Select (Q)" => "Seleziona (Q)",
        "Orbit (O)" => "Orbita (O)",
        "Pan (G)" => "Sposta (G)",
        "Zoom (drag up/down)" => "Zoom (trascina su/giù)",
        "Select / orbit / pan tool" => "Strumento seleziona / orbita / sposta",
        "Solid" => "Solido",
        "Rendered" => "Renderizzato",
        "Frame the model (Home)" => "Inquadra il modello (Home)",
        "Perspective / Orthographic (5)" => "Prospettiva / ortografica (5)",
        "{objects} objects · {tris} tris · {verts} verts" => "{objects} oggetti · {tris} tri · {verts} vertici",
        "{n} textures not found" => "{n} texture non trovate",
        "{n} textures not found. The model is shown without them." => {
            "{n} texture non trovate. Il modello è mostrato senza."
        }
        "Fix" => "Risolvi",
        "Info" => "Info",
        "Materials" => "Materiali",
        "Scene" => "Scena",
        "Geometry" => "Geometria",
        "X, width" => "X, larghezza",
        "Y, depth" => "Y, profondità",
        "Z, height" => "Z, altezza",
        "Name" => "Nome",
        "Projection" => "Proiezione",
        "Opened in" => "Aperto in",
        "{ms} ms" => "{ms} ms",
        "{n} textures weren't found next to the file. Point to the folder that has them." => {
            "{n} texture non trovate accanto al file. Indica la cartella che le contiene."
        }
        "Choose texture folder…" => "Scegli cartella texture…",
        "Choose texture folder" => "Scegli cartella texture",
        "Missing" => "Mancanti",
        "MISSING" => "MANCA",
        "No textures: plain material color." => "Nessuna texture: colore del materiale.",
        "Click to show this channel on the object (again to turn it off)" => {
            "Clic per mostrare questo canale sull'oggetto (di nuovo per spegnerlo)"
        }
        "{n} tri" => "{n} tri",
        "{tris} tris" => "{tris} tri",
        "opened in {ms} ms" => "aperto in {ms} ms",
        "the loader stopped" => "il caricamento si è interrotto",
        "No file open" => "Nessun file aperto",
        "Drop a model here" => "Trascina qui un modello",
        "or open it from disk. Textures are looked up in the same folder." => {
            "oppure aprilo dal disco. Le texture vengono cercate nella stessa cartella."
        }

        // Navigation presets
        "Style" => "Stile",
        "Look around" => "Guarda intorno",
        "Fly" => "Vola",
        "Fly (Shift faster)" => "Vola (Shift più veloce)",
        "Snap to the nearest view" => "Aggancia alla vista più vicina",
        "Shift while rotating" => "Shift mentre ruoti",
        "LMB  /  RMB drag" => "LMB  /  RMB trascina",
        "RMB drag" => "RMB trascina",
        "Welcome to PolyLoupe" => "Benvenuto in PolyLoupe",
        "How do you move around in 3D? Pick the app your hands already know; you can change it later in Preferences." => {
            "Come ti muovi in 3D? Scegli l'app che le tue mani conoscono già; puoi cambiarla dopo nelle Preferenze."
        }
        "Start" => "Inizia",

        // Preferences
        "Preferences" => "Preferenze",
        "Interface" => "Interfaccia",
        "Language" => "Lingua",
        "System" => "Sistema",
        "Files" => "File",
        "Open files in the same window" => "Apri i file nella stessa finestra",
        "Opening a file from Explorer loads it in the window that's already open instead of starting a new one" => {
            "Aprire un file da Esplora file lo carica nella finestra già aperta invece di avviarne una nuova"
        }
        "Choose file types…" => "Scegli i tipi di file…",
        "Opens Windows Default apps, where PolyLoupe can open .glb, .gltf, .fbx, .obj and .stl" => {
            "Apre App predefinite di Windows, dove PolyLoupe può aprire .glb, .gltf, .fbx, .obj e .stl"
        }
        "Viewport" => "Vista 3D",
        "Image Export" => "Esportazione immagine",
        "Resolution" => "Risoluzione",
        "Multiplies the viewport size, up to 4096 px on the long side" => {
            "Moltiplica la dimensione della vista, fino a 4096 px sul lato lungo"
        }
        "Transparent background" => "Sfondo trasparente",
        "Transparent" => "Trasparente",
        "Transparent background with checkerboard preview and alpha export" => {
            "Sfondo trasparente con anteprima a scacchiera ed esportazione alpha"
        }
        "Export with alpha transparency (best with GIF)" => {
            "Esporta con trasparenza alpha (consigliato con GIF)"
        }
        "MP4 does not support transparency; export as GIF for transparent alpha." => {
            "Il formato MP4 non supporta la trasparenza; esporta come GIF per la trasparenza alpha."
        }
        "Include the floor grid" => "Includi la griglia a pavimento",
        "Close" => "Chiudi",

        // Render tab & Lighting
        "Render" => "Render",
        "Size:" => "Dimensione:",
        "Standard 4× MSAA. Fastest rendering." => "Standard 4× MSAA. Rendering più rapido.",
        "2× Supersampling (SSAA) + Lanczos3 filter. Razor-sharp speculars and edges." => {
            "2× Supersampling (SSAA) + filtro Lanczos3. Riflessi e bordi nitidi senza aliasing."
        }
        "4× Ultra Supersampling. Master resolution for prints and fine details." => {
            "4× Ultra Supersampling. Risoluzione master per stampe e dettagli fini."
        }
        "Framing guide (Passepartout)" => "Guida inquadratura (Passepartout)",
        "Darkens regions outside the render aspect ratio in the viewport" => {
            "Scurisce le aree esterne al formato di rendering nel viewport"
        }
        "Export PNG with alpha transparency (disables background sky)" => {
            "Esporta PNG con trasparenza alpha (disattiva lo sfondo)"
        }
        "Include floor grid" => "Includi griglia del piano",
        "Render and save image (F12)" => "Esegue il render e salva l'immagine (F12)",
        "Render Image…" => "Render immagine…",
        "Open a file to render." => "Apri un file per effettuare il render.",
        "Tracks the brightest point in the sky; turns with Environment rotation." => {
            "Segue il punto più luminoso del cielo; ruota con l'ambiente."
        }
        "Angle (Yaw)" => "Angolo (Yaw)",
        "Elevation (Pitch)" => "Elevazione (Pitch)",
        "Intensity" => "Intensità",
        "Light color" => "Colore luce",
        "Model shadows" => "Ombre del modello",
        "Primary light casts shadows on the model" => "La luce principale proietta ombre sul modello",
        "Enable Fill Light" => "Abilita luce di riempimento",
        "Add a secondary studio light for rim or fill illumination" => {
            "Aggiunge una luce secondaria da studio per controluce o riempimento"
        }
        "MP4 needs ffmpeg on PATH (winget install ffmpeg). GIF works natively." => {
            "MP4 richiede ffmpeg nel PATH (winget install ffmpeg). Le GIF funzionano nativamente."
        }
        "Play clip during turntable" => "Riproduci l'animazione durante la rotazione",
        "Encoding…" => "Codifica in corso…",
        "Output: {w} × {h} px" => "Risoluzione: {w} × {h} px",
        "Lights & Shadows configured in Render tab" => "Luci e ombre configurate nella tab Render",
        "Primary Light" => "Luce principale",
        "Follow HDRI" => "Segui HDRI",
        "Pure White" => "Bianco puro",
        "Warm 3200K" => "Calda 3200K",
        "Daylight 5500K" => "Daylight 5500K",
        "Cool 6500K" => "Fredda 6500K",
        "Golden Hour" => "Golden Hour",
        "Secondary / Fill Light" => "Luce secondaria / Riempimento",
        "Turntable (360° Animation)" => "Turntable (Animazione 360°)",
        "Shading Mode" => "Modalità ombreggiatura",
        "1080p FHD (16:9)" => "1080p FHD (16:9)",
        "1440p 2K (16:9)" => "1440p 2K (16:9)",
        "2160p 4K (16:9)" => "2160p 4K (16:9)",
        "1080p Square (1:1)" => "1080p Quadrato (1:1)",
        "2048p Square (1:1)" => "2048p Quadrato (1:1)",
        "Portrait 4:5 (1080×1350)" => "Portrait 4:5 (1080×1350)",
        "Viewport (1×)" => "Viewport (1×)",
        "Viewport (2×)" => "Viewport (2×)",
        "Quality (SSAA)" => "Qualità (SSAA)",
        "Solid + Wire" => "Solido + Wire",
        "Split view: compare two shading styles on this model" => {
            "Vista divisa: confronta due stili di visualizzazione su questo modello"
        }
        "Gradient" => "Gradiente",
        "Show light gizmos in viewport" => "Mostra gizmo luci nel viewport",
        "Display interactive light handles in the 3D viewport (Photo Mode)" => {
            "Mostra manipolatori luce interattivi nel viewport 3D (Modalità Foto)"
        }
        "Studio Lights" => "Luci studio",
        "+ Add Light" => "+ Aggiungi luce",
        "Add a new custom light source (up to 6)" => "Aggiunge una nuova sorgente di luce personalizzata (fino a 6)",
        "Key Light" => "Luce principale",
        "Fill Light" => "Luce di riempimento",
        "Remove this light" => "Rimuovi questa luce",
        "Presets:" => "Preset:",
        "Compare Shading Styles…" => "Confronta stili di visualizzazione…",
        "Bounding box" => "Bounding box",
        "Show model dimensions and bounding box axes" => "Mostra le dimensioni del modello e gli assi del bounding box",
        "Color picker" => "Selettore colore",
        "Opacity" => "Opacità",
        "Style B: Wireframe" => "Stile B: Wireframe",
        "Style B: Solid + Wireframe" => "Stile B: Solido + Wireframe",
        "Style B: Solid" => "Stile B: Solido",
        "Style B: Rendered" => "Stile B: Rendered",
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every literal passed to `tr(...)` / `trf(...)` in the sources has an Italian entry.
    #[test]
    fn every_key_is_translated() {
        let mut missing = Vec::new();
        let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/src");
        for path in rust_files(std::path::Path::new(dir)) {
            let text = std::fs::read_to_string(&path).unwrap();
            for key in literal_args(&text) {
                if italian_text(&key).is_none() {
                    missing.push(format!("{}: {key:?}", path.display()));
                }
            }
        }
        assert!(missing.is_empty(), "missing Italian translations:\n{}", missing.join("\n"));
    }

    fn rust_files(dir: &std::path::Path) -> Vec<std::path::PathBuf> {
        let mut out = Vec::new();
        for entry in std::fs::read_dir(dir).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                out.extend(rust_files(&path));
            } else if path.extension().is_some_and(|e| e == "rs") && !path.ends_with("i18n.rs") {
                out.push(path);
            }
        }
        out
    }

    /// String literals right after `tr(` or `trf(`, unescaped.
    fn literal_args(text: &str) -> Vec<String> {
        let mut out = Vec::new();
        for pattern in ["tr(\"", "trf(\""] {
            let mut rest = text;
            while let Some(i) = rest.find(pattern) {
                let prev = rest[..i].chars().last();
                rest = &rest[i + pattern.len()..];
                if prev.is_some_and(|c| c.is_alphanumeric() || c == '_') {
                    continue;
                }
                let mut key = String::new();
                let mut chars = rest.chars();
                while let Some(c) = chars.next() {
                    match c {
                        '"' => break,
                        '\\' => match chars.next() {
                            Some('n') => key.push('\n'),
                            Some(other) => key.push(other),
                            None => {}
                        },
                        c => key.push(c),
                    }
                }
                out.push(key);
            }
        }
        out
    }
}
