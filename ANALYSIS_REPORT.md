# Análisis Completo del Código Base de Niri
## Informe FASE 0: Transformación a i3-clone

**Fecha**: 2025-10-03
**Objetivo**: Mapear completamente la arquitectura de niri para planificar su transformación en un clon de i3wm

---

## 1. ARQUITECTURA ACTUAL DE LAYOUT

### Jerarquía de Estructuras

```
Layout (src/layout/mod.rs)
  └─ Monitor[] (src/layout/monitor.rs)
      └─ Workspace[] (src/layout/workspace.rs)
          ├─ ScrollingSpace (src/layout/scrolling.rs) ← NÚCLEO DEL SCROLLING
          │   └─ Column[]
          │       └─ Tile[]
          └─ FloatingSpace (src/layout/floating.rs)
              └─ FloatingTile[]
```

### Módulo de Layout (22,183 líneas totales)

**Archivos principales:**
- `scrolling.rs` - 195,013 bytes - **NÚCLEO A ELIMINAR**
- `workspace.rs` - 63,871 bytes - Gestión de workspaces (reformar)
- `monitor.rs` - 76,454 bytes - Gestión de monitores (reformar)
- `mod.rs` - 174,052 bytes - Layout top-level (reformar)
- `tile.rs` - 48,986 bytes - Individual tiles (mantener/adaptar)
- `floating.rs` - 46,542 bytes - Floating windows (adaptar a i3)
- `tests.rs` - 111,792 bytes - Tests randomizados (reescribir)

**Archivos auxiliares:**
- `closing_window.rs` - Animaciones de cierre (mantener)
- `opening_window.rs` - Animaciones de apertura (mantener)
- `focus_ring.rs` - Indicador de foco (mantener)
- `shadow.rs` - Sombras (mantener)
- `tab_indicator.rs` - Indicador de tabs (adaptar)
- `insert_hint_element.rs` - Hints de inserción (adaptar)

---

## 2. COMPONENTES DEL SCROLLING A ELIMINAR

### 2.1 Archivo Principal: `src/layout/scrolling.rs`

**Estructuras clave a eliminar:**
```rust
pub struct ScrollingSpace<W: LayoutElement> {
    columns: Vec<Column<W>>,           // ← Reemplazar con árbol
    view_offset: ViewOffset,           // ← ELIMINAR
    activate_prev_column_on_removal: Option<f64>, // ← ELIMINAR
    view_offset_before_fullscreen: Option<f64>,   // ← ELIMINAR
    // ... mantener algunas propiedades básicas
}

pub enum ViewOffset {
    Static(f64),        // ← ELIMINAR
    Animation(Animation), // ← ELIMINAR
    Gesture(ViewGesture), // ← ELIMINAR
}

pub struct ViewGesture {
    current_view_offset: f64,      // ← ELIMINAR
    tracker: SwipeTracker,         // ← ELIMINAR
    delta_from_tracker: f64,       // ← ELIMINAR
    stationary_view_offset: f64,   // ← ELIMINAR
    // ...
}

pub struct Column<W: LayoutElement> {
    tiles: Vec<Tile<W>>,  // ← Mantener concepto pero cambiar estructura
    // ...
}
```

**Constantes del scrolling:**
```rust
const VIEW_GESTURE_WORKING_AREA_MOVEMENT: f64 = 1200.; // ← ELIMINAR
```

**Métodos a eliminar/reformar (ejemplos):**
- `view_offset()` - gestión del offset de vista
- `view_pos()` - posición de vista
- `scroll_amount_to_activate()` - scroll para activar
- `advance_animations()` - animaciones de scroll
- `view_offset_gesture_begin()` - inicio de gesture de scroll
- `view_offset_gesture_update()` - actualización de gesture

### 2.2 Referencias en `src/layout/workspace.rs`

**Usos de ScrollingSpace (7 ocurrencias):**
```rust
use super::scrolling::{
    Column, ColumnWidth, ScrollDirection, ScrollingSpace, ScrollingSpaceRenderElement,
};

pub struct Workspace<W: LayoutElement> {
    scrolling: ScrollingSpace<W>,  // ← Reemplazar con árbol de contenedores
    floating: FloatingSpace<W>,     // Mantener
    // ...
}
```

### 2.3 Referencias en `src/layout/monitor.rs`

**Constantes de workspace scrolling:**
```rust
const WORKSPACE_GESTURE_MOVEMENT: f64 = 300.;          // ← Mantener (para switch)
const WORKSPACE_DND_EDGE_SCROLL_MOVEMENT: f64 = 1500.; // ← Mantener (para switch)
```

### 2.4 Referencias en Input Handling

**Archivos con lógica de scroll:**
- `src/input/mod.rs` (3 ocurrencias ScrollDirection/ViewOffset)
- `src/input/scroll_swipe_gesture.rs` - Gestos de scroll ← **ELIMINAR**
- `src/input/scroll_tracker.rs` - Tracker de scroll ← **ELIMINAR**
- `src/input/spatial_movement_grab.rs` (5 referencias)
- `src/input/touch_overview_grab.rs` (4 referencias)

---

## 3. CONFIGURACIÓN (niri-config/)

### 3.1 Binds relacionados con scrolling

**En `niri-config/src/binds.rs`:**

Acciones relacionadas con columnas (reformar para contenedores):
```rust
FocusColumnLeft,
FocusColumnRight,
FocusColumnFirst,
FocusColumnLast,
MoveColumnLeft,
MoveColumnRight,
MoveColumnToFirst,
MoveColumnToLast,
ConsumeOrExpelWindowLeft,
ConsumeOrExpelWindowRight,
ConsumeWindowIntoColumn,
ExpelWindowFromColumn,
SwapWindowLeft,
SwapWindowRight,
ToggleColumnTabbedDisplay,
CenterColumn,
CenterVisibleColumns,
```

Acciones de scroll a eliminar:
```rust
// Buscar en config:
// - ScrollWheel bindings
// - TouchpadScroll bindings
// - CenterFocusedColumn behavior
```

### 3.2 Layout Config

**En `niri-config/src/layout.rs`:**
```rust
pub struct Layout {
    // Opciones relacionadas con scrolling/columnas
    pub center_focused_column: CenterFocusedColumn, // ← Eliminar
    // ... otras opciones a revisar
}

pub enum CenterFocusedColumn {
    Never,
    Always,
    OnOverflow,
}
```

---

## 4. IPC (niri-ipc/)

### 4.1 Estructuras IPC Actuales

**En `niri-ipc/src/lib.rs`:**

```rust
pub struct Window {
    pub id: u64,
    pub title: Option<String>,
    pub app_id: Option<String>,
    pub workspace_id: u64,
    pub is_focused: bool,
    pub layout: WindowLayout,  // ← Contiene info de columna/tile
}

pub struct WindowLayout {
    /// (column_index, tile_index_in_column) - 1-based
    pub tiled: Option<(usize, usize)>,  // ← REFORMAR a árbol
    pub floating: Option<Rectangle<i32, Logical>>,
}

pub struct Workspace {
    pub id: u64,
    pub idx: Option<u32>,
    pub name: Option<String>,
    pub output: Option<String>,
    pub is_active: bool,
    pub is_focused: bool,
    pub active_window_id: Option<u64>,
}

pub struct Output {
    pub name: String,
    pub make: String,
    pub model: String,
    pub physical_size: Option<(u32, u32)>,
    pub modes: Vec<Mode>,
    pub current_mode: Option<Mode>,
    pub vrr_supported: bool,
    pub vrr_enabled: bool,
    pub logical: LogicalOutput,
}
```

### 4.2 Acciones IPC que dependen de columnas

**Acciones a reformar para i3:**
```rust
pub enum Action {
    // Acciones de columna → convertir a contenedor
    FocusColumnLeft {},
    FocusColumnRight {},
    FocusColumn { index: usize },
    MoveColumnLeft {},
    MoveColumnRight {},

    // Acciones de window en columna → adaptar
    FocusWindowInColumn { index: u8 },
    MoveWindowDown {},
    MoveWindowUp {},
    ConsumeOrExpelWindowLeft {},
    ConsumeOrExpelWindowRight {},

    // Workspaces - mantener estructura base
    FocusWorkspace(WorkspaceReferenceArg),
    FocusWorkspacePrevious {},
    MoveWindowToWorkspace(WorkspaceReferenceArg),

    // Estas probablemente se mantienen
    CloseWindow { id: Option<u64> },
    FullscreenWindow { id: Option<u64> },
    FocusWindow { id: u64 },

    // ... más acciones
}
```

---

## 5. COMPONENTES REUTILIZABLES

### 5.1 Mantener (sin cambios mayores)

1. **Backend smithay** (`src/backend/`)
   - `tty.rs` - Backend DRM/TTY
   - `winit.rs` - Backend nested
   - `headless.rs` - Backend headless

2. **Handlers Wayland** (`src/handlers/`)
   - `compositor.rs` - Handler de compositor
   - `xdg_shell.rs` - XDG shell (mantener mayoría)
   - `layer_shell.rs` - Layer shell

3. **Protocolos** (`src/protocols/`)
   - Mayoría de protocolos se mantienen

4. **Rendering** (`src/render_helpers/`)
   - Sistema de rendering actual

5. **Input básico** (`src/input/mod.rs`)
   - Keyboard, mouse handling básico
   - **ELIMINAR**: scroll gestures específicos

6. **Window management básico**
   - `src/window/mod.rs` - Wrapper de ventanas
   - Tile rendering (`src/layout/tile.rs` parcial)

### 5.2 Reformar Completamente

1. **Layout system** (`src/layout/`)
   - `mod.rs` - Layout top-level → árbol de contenedores
   - `scrolling.rs` - **ELIMINAR COMPLETAMENTE**
   - `workspace.rs` - Reformar para workspaces discretos
   - `monitor.rs` - Adaptar para múltiples monitores i3
   - `tests.rs` - Reescribir para nueva lógica

2. **Config parsing** (`niri-config/`)
   - Crear nuevo parser KDL → i3 config
   - O mantener KDL con sintaxis i3-like

3. **IPC** (`niri-ipc/`)
   - Reformar estructuras para árbol
   - Implementar i3-ipc compatible protocol
   - Cambiar de JSON newline-delimited a i3 JSON-RPC

---

## 6. ESTADÍSTICAS DE CÓDIGO

### Impacto del Scrolling en el Codebase

**Archivos con referencias a "scroll" (26 archivos):**
- Layout: 7 archivos (mod, scrolling, workspace, monitor, tile, tests, floating)
- Input: 5 archivos (mod, scroll_swipe_gesture, scroll_tracker, touch_overview, spatial_movement)
- Config: 4 archivos (binds, gestures, input, lib)
- IPC: 1 archivo (lib)
- Otros: 9 archivos (window, protocols, dbus, etc.)

**Ocurrencias de tipos core del scrolling:**
- `ScrollDirection | ViewOffset | ScrollingSpace`: **86 ocurrencias** en 8 archivos

**Líneas de código:**
- Layout total: ~22,000 líneas
- `scrolling.rs`: ~5,000 líneas ← **ELIMINAR**
- Tests de layout: ~4,000 líneas ← **REESCRIBIR**
- Input scroll: ~500 líneas ← **ELIMINAR**

---

## 7. PLAN DE ELIMINACIÓN DEL SCROLLING

### Fase 1A: Preparación
1. Backup del repositorio
2. Crear branch `i3-conversion`
3. Documentar todas las dependencias

### Fase 1B: Eliminación de Scrolling Core

**Orden de eliminación:**

1. **Input handlers** (bajo acoplamiento):
   ```
   src/input/scroll_swipe_gesture.rs  → DELETE
   src/input/scroll_tracker.rs        → DELETE
   ```

2. **Config bindings** (medio acoplamiento):
   ```
   niri-config/src/binds.rs:
     - Eliminar Trigger::WheelScroll*
     - Eliminar Trigger::TouchpadScroll*
     - Comentar/eliminar acciones de Column*
   ```

3. **IPC Actions** (medio acoplamiento):
   ```
   niri-ipc/src/lib.rs:
     - Comentar acciones de Column
     - Preparar WindowLayout para árbol
   ```

4. **LayoutScrollingSpace** (alto acoplamiento):
   ```
   src/layout/scrolling.rs:
     - Marcar como deprecated
     - Extraer solo Column/Tile structs
     - Comentar ViewOffset y métodos de scroll
   ```

5. **Workspace integration** (alto acoplamiento):
   ```
   src/layout/workspace.rs:
     - Comentar uso de ScrollingSpace
     - Preparar para nueva estructura
   ```

6. **Monitor integration** (alto acoplamiento):
   ```
   src/layout/monitor.rs:
     - Limpiar gestos de scroll de workspace
     - Mantener solo switch discreto
   ```

### Fase 1C: Stub Temporal

Crear estructura temporal para que compile:
```rust
// src/layout/tiling.rs (TEMPORAL)
pub struct TilingSpace<W: LayoutElement> {
    windows: Vec<Tile<W>>,  // Simplificado temporalmente
    // TODO: Reemplazar con árbol de contenedores en Fase 2
}
```

---

## 8. ARQUITECTURA OBJETIVO (i3)

### 8.1 Nueva Jerarquía

```
Layout
  └─ Output[]
      └─ Workspace[] (discretos: 1-10 + nombrados)
          ├─ Container (tree jerárquico)
          │   ├─ SplitContainer { orientation, children, ratio }
          │   ├─ StackedContainer { children }
          │   ├─ TabbedContainer { children }
          │   └─ WindowContainer { surface }
          └─ FloatingContainer
              └─ FloatingWindow[]
```

### 8.2 Nuevas Estructuras Propuestas

```rust
pub enum Container<W: LayoutElement> {
    Root {
        children: Vec<Container<W>>,
    },
    Workspace {
        id: WorkspaceId,
        name: Option<String>,
        layout: ContainerLayout,
        children: Vec<Container<W>>,
    },
    Split {
        orientation: Orientation,
        ratio: Vec<f64>,  // Ratios de cada hijo
        children: Vec<Container<W>>,
    },
    Stacked {
        active_idx: usize,
        children: Vec<Container<W>>,
    },
    Tabbed {
        active_idx: usize,
        children: Vec<Container<W>>,
    },
    Window {
        tile: Tile<W>,
        fullscreen: bool,
    },
}

pub enum Orientation {
    Horizontal,  // i3: splitv
    Vertical,    // i3: splith
}

pub enum ContainerLayout {
    SplitH,
    SplitV,
    Stacked,
    Tabbed,
}
```

---

## 9. ESTIMACIÓN DE ESFUERZO

### Líneas de código a modificar/eliminar

| Componente | Acción | Líneas | Esfuerzo |
|------------|--------|--------|----------|
| `scrolling.rs` | ELIMINAR | ~5,000 | Alto |
| `workspace.rs` | REFORMAR | ~2,000 | Alto |
| `monitor.rs` | REFORMAR | ~2,500 | Medio |
| `mod.rs` (layout) | REFORMAR | ~5,000 | Alto |
| Input handlers | ELIMINAR | ~500 | Bajo |
| Config parsing | REFORMAR | ~1,000 | Medio |
| IPC types | REFORMAR | ~500 | Medio |
| Tests | REESCRIBIR | ~4,000 | Alto |
| **TOTAL** | | **~20,500** | **MUY ALTO** |

### Fases estimadas

- **FASE 1** (Eliminación): 2-3 días
- **FASE 2** (Container tree): 5-7 días
- **FASE 3** (Workspaces discretos): 2-3 días
- **FASE 4** (Config parser i3): 3-4 días
- **FASE 5** (IPC i3): 3-4 días
- **FASE 6** (Window rules): 2-3 días
- **FASE 7** (Floating): 2-3 días
- **FASE 8** (Testing): 3-5 días

**Total estimado**: 22-32 días de desarrollo intensivo

---

## 10. RIESGOS Y CONSIDERACIONES

### Riesgos Técnicos

1. **Acoplamiento con smithay**: Algunas APIs de smithay pueden asumir ciertos patrones
2. **Rendering pipeline**: Puede necesitar ajustes para layouts no-scroll
3. **Animaciones**: Sistema de animaciones actual optimizado para scroll
4. **Tests**: Suite de tests actual muy acoplada a scrolling

### Beneficios del Approach

1. **Mantener smithay**: Mejor que empezar desde cero o usar wlroots
2. **Rendering maduro**: Sistema de rendering ya probado
3. **Protocolos**: Soporte de protocolos Wayland ya implementado
4. **Input handling**: Mayor parte reutilizable

### Alternativas Consideradas

1. **Fork de sway**: Requiere aprender wlroots, licencia diferente
2. **Desde cero con smithay**: Mucho más esfuerzo
3. **Modificar i3 para Wayland**: Código C muy acoplado a X11

---

## 11. PRÓXIMOS PASOS RECOMENDADOS

### Inmediatos (antes de FASE 1)

1. ✅ Crear este análisis
2. ⏳ Revisar y aprobar plan
3. ⏳ Crear branch `i3-conversion`
4. ⏳ Setup de entorno de testing
5. ⏳ Leer código de i3 para entender detalles de container tree

### Decisiones Pendientes

1. **Config format**: ¿KDL con sintaxis i3-like o parser i3 nativo?
2. **IPC protocol**: ¿JSON-RPC exacto de i3 o wrapper compatible?
3. **Floating**: ¿Usar FloatingSpace actual o reescribir?
4. **Animaciones**: ¿Mantener animaciones de niri o eliminarlas para ser más i3-like?

---

## CONCLUSIÓN

El código base de niri está **bien estructurado** pero **fuertemente acoplado** al paradigma de scrolling. La eliminación requerirá:

1. **Eliminar ~5,500 líneas** de código de scrolling
2. **Reformar ~15,000 líneas** de layout/workspace/monitor
3. **Reescribir ~4,000 líneas** de tests
4. **Adaptar ~2,000 líneas** de config/IPC

**Total**: ~26,500 líneas afectadas de un total de ~50,000 líneas del proyecto.

Es un proyecto **ambicioso pero factible**, especialmente porque:
- La base de smithay es sólida
- El sistema de rendering ya funciona
- Los handlers de Wayland están completos
- La estructura modular facilita el refactor

**Recomendación**: Proceder con FASE 1 (Eliminación de scrolling) de forma incremental, asegurando que el proyecto compile en cada paso.
