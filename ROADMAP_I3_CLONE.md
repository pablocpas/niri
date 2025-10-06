# ROADMAP COMPLETO: Convertir niri en clon exacto de i3/sway

## 📋 ÍNDICE
- [Estado Actual](#estado-actual)
- [Características Pendientes](#características-pendientes)
- [Prioridades de Implementación](#prioridades-de-implementación)
- [Notas Técnicas](#notas-técnicas)

---

## ✅ ESTADO ACTUAL (Ya Implementado)

### Sistema de Contenedores Core
- ✅ Árbol jerárquico de contenedores usando SlotMap
- ✅ Contenedores con Layout: SplitH, SplitV, Tabbed, Stacked
- ✅ Nodos internos (containers) y hojas (tiles con ventanas)
- ✅ Navegación direccional: focus left/right/up/down
- ✅ Movimiento de ventanas: move left/right/up/down
- ✅ Split horizontal: `split-h`
- ✅ Split vertical: `split-v`
- ✅ Focus parent: `focus-parent`
- ✅ Focus child: `focus-child`
- ✅ Fullscreen básico funcional
- ✅ Sistema de floating windows
- ✅ Cambio de layout mode: `set-layout-mode`

---

## 🔴 CARACTERÍSTICAS PENDIENTES (205+ tareas)

### **1. GESTIÓN DE WORKSPACES** (20 tareas)

#### 1.1 Workspaces Independientes por Monitor
- [ ] Desacoplar workspaces de monitores (actualmente están vinculados)
- [ ] Cada monitor tiene su propio conjunto de workspaces
- [ ] Workspaces pueden existir sin monitor asignado
- [ ] Migrar workspace a otro monitor automáticamente si se desconecta

#### 1.2 Creación Dinámica de Workspaces
- [ ] `workspace number N` - crear workspace si no existe
- [ ] Auto-crear workspace al navegar con número
- [ ] Workspaces numerados: 1, 2, 3, ... 10+
- [ ] Validación de nombres de workspace

#### 1.3 Workspaces con Nombre
- [ ] `workspace "nombre"` - workspace solo con nombre
- [ ] `workspace N:nombre` - número + nombre descriptivo
- [ ] Renombrar workspace en tiempo real: `rename workspace to "nuevo"`
- [ ] Mostrar nombre en bar y en tree

#### 1.4 Navegación de Workspaces
- [ ] `workspace back_and_forth` - alternar con último workspace
- [ ] `workspace next` - ir al siguiente workspace
- [ ] `workspace prev` - ir al anterior workspace
- [ ] `workspace next_on_output` - siguiente en mismo monitor
- [ ] `workspace prev_on_output` - anterior en mismo monitor
- [ ] Navegación cíclica (del último volver al primero)

#### 1.5 Limpieza Automática
- [ ] Eliminar workspaces vacíos automáticamente (configurable)
- [ ] `workspace_auto_back_and_forth yes/no` en config
- [ ] Preservar workspace si tiene nombre asignado
- [ ] Opción: mantener siempre workspace 1-10

#### 1.6 Asignación y Movimiento
- [ ] `assign [class="Firefox"] workspace "3:web"`
- [ ] `for_window [app_id="..."] move to workspace N`
- [ ] Mover workspace a otro output: `move workspace to output <name>`
- [ ] `move workspace to output left/right/up/down`

---

### **2. COMANDOS DE MOVIMIENTO Y FOCUS** (25 tareas)

#### 2.1 Movimiento Preciso
- [ ] `move left N px` - mover N pixels a la izquierda
- [ ] `move right N px`
- [ ] `move up N px`
- [ ] `move down N px`
- [ ] `move left N ppt` - mover N% del contenedor padre

#### 2.2 Posicionamiento Absoluto (Floating)
- [ ] `move position center` - centrar ventana floating
- [ ] `move position mouse` - mover a posición del mouse
- [ ] `move absolute position X px Y px`
- [ ] `move position X px Y px` (relativo a workspace)

#### 2.3 Intercambio y Swap
- [ ] `swap container with mark <mark>`
- [ ] `swap container with id <con_id>`
- [ ] `swap container with con_id <id>`
- [ ] Swap preservando geometría de ambos contenedores

#### 2.4 Focus Avanzado
- [ ] `focus output <output>` - dar foco a un monitor
- [ ] `focus mode_toggle` - alternar foco entre floating/tiling
- [ ] `focus next` - siguiente ventana (orden de apertura)
- [ ] `focus prev` - ventana anterior
- [ ] `focus urgent` - ir a primera ventana con urgency
- [ ] Focus con criterios: `[class="Firefox"] focus`
- [ ] `focus next sibling` - siguiente hermano en árbol
- [ ] `focus prev sibling` - hermano anterior

#### 2.5 Movimiento a Marcas
- [ ] `move window to mark <mark>`
- [ ] `move container to mark <mark>`
- [ ] Posicionar junto a la marca (next to)

#### 2.6 Movimiento sin Cambiar Focus
- [ ] `move container to workspace current` (mover sin cambiar de workspace)
- [ ] `move window to workspace N, workspace N` (mover y seguir)
- [ ] `move window to workspace N` sin cambiar de workspace

---

### **3. SCRATCHPAD** (12 tareas)

#### 3.1 Workspace Scratchpad
- [ ] Workspace especial invisible para scratchpad
- [ ] No aparece en lista de workspaces normales
- [ ] Persiste entre reinicios (opcional)

#### 3.2 Comandos Básicos
- [ ] `move scratchpad` - mover ventana actual a scratchpad
- [ ] `scratchpad show` - mostrar/ocultar última ventana scratchpad
- [ ] Ocultar scratchpad al perder foco (configurable)

#### 3.3 Múltiples Ventanas
- [ ] Rotar entre ventanas scratchpad con `scratchpad show`
- [ ] Mantener orden LIFO de ventanas
- [ ] `scratchpad show [criterios]` - mostrar ventana específica

#### 3.4 Posicionamiento
- [ ] Scratchpad centrado por defecto
- [ ] Tamaño configurable: 80% width, 80% height
- [ ] `for_window [criteria] move scratchpad, scratchpad show`
- [ ] Recordar posición/tamaño individual por ventana

#### 3.5 Comportamiento Sticky
- [ ] Scratchpad visible en todos los workspaces (sticky)
- [ ] Seguir al workspace activo
- [ ] Toggle sticky per-window

---

### **4. MARCAS Y CRITERIOS** (15 tareas)

#### 4.1 Sistema de Marcas
- [ ] `mark <identifier>` - marcar contenedor actual
- [ ] `mark --add <identifier>` - añadir marca (no reemplazar)
- [ ] `mark --replace <identifier>` - reemplazar marca existente
- [ ] `mark --toggle <identifier>` - toggle marca
- [ ] `unmark <identifier>` - quitar marca específica
- [ ] `unmark` - quitar todas las marcas del contenedor

#### 4.2 Navegación con Marcas
- [ ] `[con_mark="mark"] focus`
- [ ] `[con_mark="mark"] move to workspace N`
- [ ] Swap con marca (ver sección 2.3)

#### 4.3 Visualización
- [ ] Mostrar marcas visualmente en ventanas/borders
- [ ] Indicador de marca en bar
- [ ] Color configurable para marcas

#### 4.4 Persistencia
- [ ] Guardar marcas entre reinicios
- [ ] Restaurar marcas al hacer `restart`

#### 4.5 Criterios de Selección
- [ ] `[title="regex"]` - por título de ventana
- [ ] `[class="regex"]` - por clase X11 / app_id Wayland
- [ ] `[instance="regex"]` - por instance
- [ ] `[window_role="regex"]` - por window role X11
- [ ] `[window_type="type"]` - por tipo: normal, dialog, utility, toolbar, splash, menu, dropdown_menu, popup_menu, tooltip, notification
- [ ] `[workspace="name"]` - ventanas en workspace específico
- [ ] `[con_id="id"]` - por ID único de contenedor
- [ ] `[con_mark="mark"]` - por marca
- [ ] `[urgent="yes/no"]` - ventanas urgentes
- [ ] `[floating]` - solo ventanas floating
- [ ] `[tiling]` - solo ventanas tiling
- [ ] Criterios combinados: `[class="Firefox" title=".*YouTube.*"] fullscreen enable`
- [ ] Negación: `[class="Firefox"] [title!="Private"]`
- [ ] Comodines: `[class=".*"]` (todas las ventanas)

---

### **5. LAYOUTS Y CONTENEDORES AVANZADOS** (22 tareas)

#### 5.1 Toggle de Layouts
- [ ] `layout toggle split` - alternar entre splith y splitv
- [ ] `layout toggle all` - rotar: splith → splitv → tabbed → stacked → splith
- [ ] `layout toggle tabbed stacking`
- [ ] `layout toggle tabbed stacked splith splitv`
- [ ] Layout toggle configurable: `layout toggle split tabbed`

#### 5.2 Split Automático
- [ ] Auto-detección de orientación split basado en aspect ratio
- [ ] `default_orientation horizontal/vertical/auto` en config
- [ ] Threshold configurable para auto-split
- [ ] `split toggle` - cambiar orientación del próximo split

#### 5.3 Indicadores Visuales
- [ ] Mostrar split orientation visualmente
- [ ] Línea/indicador entre splits
- [ ] Color de indicador configurable

#### 5.4 Tabbed Layout
- [ ] Pestañas con título de ventana
- [ ] Tab bar posición: top/bottom/left/right
- [ ] Tab width: fixed/auto
- [ ] Tab close button (opcional)
- [ ] Scroll tabs si no caben

#### 5.5 Stacked Layout
- [ ] Title bars completas para cada ventana
- [ ] Height de title bar configurable
- [ ] Mostrar clase/título en stacked
- [ ] Indicador de ventana activa

#### 5.6 Comandos de Gestión
- [ ] `kill` - cerrar ventana enfocada
- [ ] `kill` con criterios: `[class=".*"] kill`
- [ ] `kill window` vs `kill container` (toda la rama)

#### 5.7 Workspace Layout por Defecto
- [ ] `workspace_layout default/stacking/tabbed` en config
- [ ] Por-workspace layout: `workspace "1" layout tabbed`
- [ ] Aplicar layout al crear workspace

---

### **6. RESIZE** (15 tareas)

#### 6.1 Modo Resize Interactivo
- [ ] `mode "resize"` con bindings específicos
- [ ] Indicador visual del modo activo
- [ ] Exit automático o manual del modo

#### 6.2 Comandos de Resize
- [ ] `resize grow left N px`
- [ ] `resize grow right N px`
- [ ] `resize grow up N px`
- [ ] `resize grow down N px`
- [ ] `resize shrink left/right/up/down N px`
- [ ] `resize grow width N px`
- [ ] `resize grow height N px`

#### 6.3 Resize con Porcentajes
- [ ] `resize set width N ppt` - N% del contenedor padre
- [ ] `resize set height N ppt`
- [ ] `resize grow width N ppt`

#### 6.4 Resize Absoluto
- [ ] `resize set N px` - tamaño absoluto
- [ ] `resize set width N px height M px`
- [ ] `resize set width N ppt height M ppt`

#### 6.5 Resize de Contenedores
- [ ] Resize proporcional entre hermanos
- [ ] Redistribuir espacio al resize
- [ ] Límites mínimos/máximos por ventana

#### 6.6 Resize con Mouse
- [ ] Click+drag en bordes para resize
- [ ] Mod+Right-click para resize floating
- [ ] Snap to grid/ventanas vecinas

---

### **7. GAPS** (10 tareas)

#### 7.1 Inner Gaps
- [ ] `gaps inner all set N` - espacio entre ventanas
- [ ] `gaps inner all plus N` - incrementar
- [ ] `gaps inner all minus N` - decrementar
- [ ] `gaps inner current set/plus/minus N`

#### 7.2 Outer Gaps
- [ ] `gaps outer all set N` - espacio con bordes workspace
- [ ] `gaps outer all plus/minus N`
- [ ] `gaps outer current set/plus/minus N`

#### 7.3 Gaps Inteligentes
- [ ] `smart_gaps on/off` - ocultar si solo 1 ventana
- [ ] `smart_gaps inverse` - mostrar solo si 1 ventana
- [ ] Gaps por workspace: `workspace "1" gaps inner 20`

#### 7.4 Toggle y Control
- [ ] `gaps toggle` - activar/desactivar gaps

---

### **8. BORDERS Y DECORACIONES** (18 tareas)

#### 8.1 Estilos de Border
- [ ] `border normal` - título + borde
- [ ] `border normal N` - título + borde de N pixels
- [ ] `border pixel N` - solo borde de N pixels, sin título
- [ ] `border none` - sin decoraciones
- [ ] `border toggle`

#### 8.2 Title Bar
- [ ] Mostrar título de ventana en border normal
- [ ] Title bar con clase y título
- [ ] Truncar títulos largos con "..."
- [ ] `title_format "%title"` - formato customizable
- [ ] Placeholders: `%title`, `%class`, `%instance`, `%workspace`

#### 8.3 Alineación de Título
- [ ] `title_align left` - título alineado a izquierda
- [ ] `title_align center`
- [ ] `title_align right`

#### 8.4 Colores
- [ ] Color border focused (ya existe, revisar)
- [ ] Color border unfocused (ya existe, revisar)
- [ ] Color border urgent
- [ ] Color background de title bar
- [ ] Color texto de title bar
- [ ] Colores independientes para cada estado

#### 8.5 Hide Edge Borders
- [ ] `hide_edge_borders none` - mostrar siempre
- [ ] `hide_edge_borders vertical` - ocultar bordes izq/der en edge
- [ ] `hide_edge_borders horizontal` - ocultar arriba/abajo en edge
- [ ] `hide_edge_borders both` - ocultar todos los bordes en edge
- [ ] `hide_edge_borders smart` - ocultar si solo 1 ventana

#### 8.6 Smart Borders
- [ ] `smart_borders on` - ocultar borders si solo 1 ventana visible
- [ ] `smart_borders no_gaps` - ocultar solo si no hay gaps

---

### **9. FLOATING AVANZADO** (18 tareas)

#### 9.1 Comandos Básicos
- [ ] `floating enable` - hacer ventana floating
- [ ] `floating disable` - volver a tiling
- [ ] `floating toggle`
- [ ] Validar si ya está en el modo correcto

#### 9.2 Sticky Windows
- [ ] `sticky enable` - ventana sigue al workspace
- [ ] `sticky disable`
- [ ] `sticky toggle`
- [ ] Mostrar indicador visual de sticky

#### 9.3 Posicionamiento
- [ ] `move position center` - centrar en workspace
- [ ] `move position mouse` - bajo el cursor
- [ ] `move absolute position X px Y px`
- [ ] Restricción a working area

#### 9.4 Resize Floating
- [ ] `resize set width N px height M px`
- [ ] `resize set width N ppt height M ppt` (% del output)
- [ ] Tamaños mínimos/máximos

#### 9.5 Comportamiento
- [ ] Drag con mouse para mover (Mod+Left-click)
- [ ] Resize con mouse (Mod+Right-click)
- [ ] Double-click en title bar para toggle floating
- [ ] Snap a bordes al mover
- [ ] Snap a otras ventanas floating

#### 9.6 Memoria de Posición
- [ ] Recordar última posición floating por ventana
- [ ] Recordar último tamaño floating
- [ ] Restaurar al volver a floating

#### 9.7 For_window Rules
- [ ] `for_window [app_id="..."] floating enable`
- [ ] `for_window [window_type="dialog"] floating enable`
- [ ] Floating por defecto para diálogos

#### 9.8 Floating sobre Tiling
- [ ] Floating siempre encima (configurable)
- [ ] `floating_modifier Mod4` en config
- [ ] Z-order de floating windows

---

### **10. BARRAS Y UI** (15 tareas)

#### 10.1 Bar Nativa
- [ ] Implementar bar tipo swaybar/i3bar
- [ ] Integración con compositor
- [ ] Renderer usando cairo/pango

#### 10.2 Configuración de Bar
- [ ] `bar { }` block en config
- [ ] `position top/bottom/left/right`
- [ ] `output <name>` - bar en output específico
- [ ] Multiple bars: `bar { id "bar-1" }`

#### 10.3 Workspace Buttons
- [ ] Botones de workspace clickeables
- [ ] Mostrar número/nombre de workspace
- [ ] Highlight workspace activo
- [ ] Indicador de workspace urgente
- [ ] Color de workspace vacío/con ventanas

#### 10.4 Status Line
- [ ] `status_command <command>` - ejecutar i3status/waybar
- [ ] Parsear salida JSON
- [ ] Parsear salida texto plano
- [ ] Click events en status

#### 10.5 System Tray
- [ ] StatusNotifierItem protocol (SNI)
- [ ] Tray icons clickeables
- [ ] `tray_output <output>` - en qué monitor

#### 10.6 Binding Mode Indicator
- [ ] Mostrar modo actual: "resize", "default", etc
- [ ] Color/posición configurable
- [ ] Pango markup support

#### 10.7 Colores y Estilo
- [ ] Colors block en bar config
- [ ] `background`, `statusline`, `separator`
- [ ] `focused_workspace`, `active_workspace`, `inactive_workspace`, `urgent_workspace`
- [ ] Custom fonts: `font pango:DejaVu Sans Mono 10`

---

### **11. MODOS Y BINDING MODES** (12 tareas)

#### 11.1 Definición de Modos
- [ ] `mode "nombre" { bindings }` en config
- [ ] Múltiples modos personalizados
- [ ] Modo "default" siempre existe

#### 11.2 Cambio de Modo
- [ ] `mode "resize"` - entrar en modo resize
- [ ] `mode "default"` - volver al modo default
- [ ] Salir de modo con Escape (configurable)

#### 11.3 Bindings por Modo
- [ ] Bindings específicos en cada modo
- [ ] Override de bindings del modo default
- [ ] `--release` bindings en modos

#### 11.4 Indicador Visual
- [ ] Mostrar modo actual en bar
- [ ] Mensaje temporal en pantalla
- [ ] `mode "nombre" { bindsym ... mode "default" }` - cadena

#### 11.5 Pango Markup
- [ ] `mode --pango_markup "<b>Resize</b>"` - texto formateado
- [ ] Soporte para color/negrita/cursiva

#### 11.6 Modo Passthrough
- [ ] `mode "passthrough"` - pasar todas las teclas al cliente
- [ ] Útil para nested compositors/VMs
- [ ] Solo tecla de escape sale del modo

---

### **12. CONFIGURACIÓN Y FOR_WINDOW** (20 tareas)

#### 12.1 For_window Rules
- [ ] `for_window [criterios] comando` - reglas automáticas al abrir ventana
- [ ] Ejecutar múltiples comandos: `for_window [...] cmd1; cmd2`
- [ ] Aplicar a ventanas existentes con `reload`

#### 12.2 Ejemplos de For_window
- [ ] `for_window [class="Firefox"] layout tabbed`
- [ ] `for_window [app_id=".*"] border pixel 1`
- [ ] `for_window [title=".*Zoom.*"] floating enable`
- [ ] `for_window [window_type="dialog"] floating enable`

#### 12.3 Assign
- [ ] `assign [class="Firefox"] workspace "2:web"`
- [ ] `assign [app_id="terminal"] workspace 1`
- [ ] Mover automáticamente al abrir

#### 12.4 No Focus
- [ ] `no_focus [criteria]` - no dar foco al abrir
- [ ] Útil para notificaciones
- [ ] `for_window [window_type="notification"] no_focus`

#### 12.5 Variables
- [ ] `set $mod Mod4` - definir variables
- [ ] `set $term alacritty`
- [ ] Usar variables: `bindsym $mod+Return exec $term`
- [ ] Variables para colores: `set $bg #282828`

#### 12.6 Include
- [ ] `include <path>` - incluir otro archivo config (ya existe, verificar)
- [ ] Include con wildcards: `include ~/.config/niri/conf.d/*`
- [ ] Includes relativos al archivo actual

#### 12.7 Reload y Restart
- [ ] `reload` - recargar config sin reiniciar
- [ ] `restart` - reiniciar preservando layout
- [ ] Validación de config antes de aplicar
- [ ] Rollback si falla la carga

#### 12.8 Validación
- [ ] `niri -C <config>` - check config sin aplicar
- [ ] Mensajes de error detallados
- [ ] Warnings para deprecations

---

### **13. URGENCY Y NOTIFICACIONES** (8 tareas)

#### 13.1 Urgency Hints
- [ ] Soporte XUrgencyHint (X11)
- [ ] Soporte xdg-activation (Wayland)
- [ ] Ventana marca workspace como urgente
- [ ] Propagación de urgency al árbol

#### 13.2 Visualización
- [ ] Border color urgente (ya existe, verificar)
- [ ] Workspace button urgente en bar
- [ ] Parpadeo/animación de urgency (configurable)

#### 13.3 Navegación
- [ ] `focus urgent` - ir a primera ventana urgente
- [ ] Ordenar por tiempo de urgency

#### 13.4 Timer
- [ ] Auto-clear urgency después de N segundos
- [ ] `force_display_urgency_hint N ms` en config
- [ ] Mantener urgency hasta focus

---

### **14. MULTI-MONITOR AVANZADO** (18 tareas)

#### 14.1 Workspaces por Monitor
- [ ] Workspaces totalmente independientes por output
- [ ] Cada output mantiene su workspace activo
- [ ] Workspace puede moverse entre outputs

#### 14.2 Focus entre Outputs
- [ ] `focus output left/right/up/down`
- [ ] `focus output <name>`
- [ ] `focus output next/prev`
- [ ] Focus sigue workspace al mover

#### 14.3 Movimiento a Outputs
- [ ] `move container to output left/right/up/down`
- [ ] `move container to output <name>`
- [ ] `move workspace to output <name>`
- [ ] `move workspace to output left/right/up/down`

#### 14.4 Configuración de Outputs
- [ ] `output <name> { }` block en config
- [ ] `output <name> position X Y` - posición exacta
- [ ] `output <name> resolution WIDTHxHEIGHT@Hz`
- [ ] `output <name> scale N`
- [ ] `output <name> transform 90/180/270/flipped`
- [ ] `output <name> disable` - desactivar output

#### 14.5 Primary Output
- [ ] `primary_output <name>` - output principal
- [ ] Workspace 1 se crea en primary por defecto
- [ ] Fallback a primary si output desaparece

#### 14.6 Hot-plug
- [ ] Detectar outputs añadidos/removidos
- [ ] Mover workspaces a output disponible
- [ ] Restaurar posición de workspace al reconectar
- [ ] `workspace "1" output DP-1 eDP-1` - lista de fallback

---

### **15. IPC Y SCRIPTING** (20 tareas)

#### 15.1 Comandos IPC
- [ ] `GET_TREE` - árbol completo de contenedores
- [ ] `GET_WORKSPACES` - lista de workspaces con info
- [ ] `GET_OUTPUTS` - lista de outputs
- [ ] `GET_MARKS` - todas las marcas actuales
- [ ] `GET_VERSION` - versión del compositor
- [ ] `GET_CONFIG` - config actual
- [ ] `GET_BAR_CONFIG` - config de la bar
- [ ] `RUN_COMMAND` - ejecutar comando arbitrario
- [ ] `SEND_TICK` - enviar tick event

#### 15.2 Subscripciones a Eventos
- [ ] `SUBSCRIBE` a tipos de eventos
- [ ] Event: `workspace::focus` - cambio de workspace
- [ ] Event: `workspace::init` - workspace creado
- [ ] Event: `workspace::empty` - workspace vaciado
- [ ] Event: `workspace::urgent` - workspace urgente
- [ ] Event: `window::new` - ventana nueva
- [ ] Event: `window::close` - ventana cerrada
- [ ] Event: `window::focus` - foco cambia
- [ ] Event: `window::title` - título cambia
- [ ] Event: `window::fullscreen_mode` - fullscreen toggle
- [ ] Event: `window::move` - ventana movida
- [ ] Event: `window::floating` - floating toggle
- [ ] Event: `window::urgent` - urgency cambia
- [ ] Event: `window::mark` - marca añadida/quitada
- [ ] Event: `output::connected` - output conectado
- [ ] Event: `output::disconnected` - output desconectado
- [ ] Event: `mode` - modo cambia
- [ ] Event: `barconfig_update` - config bar cambia
- [ ] Event: `binding` - keybinding ejecutado
- [ ] Event: `tick` - tick custom

#### 15.3 Formato de Respuesta
- [ ] JSON válido en todas las respuestas
- [ ] Estructura idéntica a i3
- [ ] IDs únicos para containers

---

### **16. TÍTULOS Y TEXTO** (10 tareas)

#### 16.1 Formato de Título
- [ ] `title_format <format>` por ventana
- [ ] Placeholders: `%title` (título ventana)
- [ ] `%class` (clase/app_id)
- [ ] `%instance` (instance)
- [ ] `%workspace` (workspace actual)
- [ ] `%machine` (hostname si es remote)

#### 16.2 Alineación
- [ ] `title_align left/center/right` global
- [ ] `for_window [...] title_align center`
- [ ] Alineación en tabbed/stacked

#### 16.3 Rendering
- [ ] Pango markup support en títulos
- [ ] Truncar títulos largos con ellipsis
- [ ] Font configurable: `font pango:Monospace 10`

#### 16.4 Actualización Dinámica
- [ ] Actualizar título cuando cambia en ventana
- [ ] Event IPC al cambiar título

---

### **17. ORIENTACIÓN Y AUTO-LAYOUT** (8 tareas)

#### 17.1 Orientación por Defecto
- [ ] `default_orientation horizontal` en config
- [ ] `default_orientation vertical`
- [ ] `default_orientation auto` - según aspect ratio

#### 17.2 Auto-detect
- [ ] Threshold configurable para auto-split
- [ ] Detectar orientación óptima al abrir ventana
- [ ] Respetar orientación forzada con `split h/v`

#### 17.3 Workspace Layout
- [ ] `workspace_layout default` - splith o splitv según default_orientation
- [ ] `workspace_layout stacking` - nuevas ventanas en stacked
- [ ] `workspace_layout tabbed` - nuevas ventanas en tabbed
- [ ] Override por workspace: `workspace "1" layout tabbed`

---

### **18. FULLSCREEN** (10 tareas)

#### 18.1 Comandos
- [ ] `fullscreen enable` - activar fullscreen
- [ ] `fullscreen disable` - desactivar
- [ ] `fullscreen toggle` (ya existe, verificar)
- [ ] `fullscreen enable global` - fullscreen multi-monitor

#### 18.2 Fullscreen Global
- [ ] Cubrir todos los monitores
- [ ] Ocultar todas las bars
- [ ] Desactivar gaps/borders

#### 18.3 Comportamiento
- [ ] Preservar posición/tamaño al salir
- [ ] Fullscreen de contenedor completo (no solo ventana)
- [ ] Popups visibles encima de fullscreen (configurable)
- [ ] `popup_during_fullscreen smart/ignore/leave_fullscreen`

---

### **19. INHIBIDORES Y PERMISOS** (8 tareas)

#### 19.1 Idle Inhibit
- [ ] Soporte idle-inhibit protocol
- [ ] Ventanas pueden prevenir screen lock
- [ ] Mostrar indicador de idle inhibit

#### 19.2 Keyboard Shortcuts Inhibit
- [ ] Cliente puede inhibir shortcuts (ya existe, verificar)
- [ ] `inhibit_idle` por ventana
- [ ] Fullscreen apps inhiben por defecto (configurable)

#### 19.3 Startup Notifications
- [ ] `--no-startup-id` en `exec` para evitar cursor "loading"
- [ ] Timeout de startup notification
- [ ] Support startup-notification protocol

---

### **20. TILING AVANZADO** (15 tareas)

#### 20.1 Distribución de Espacio
- [ ] Ratios precisos entre hermanos
- [ ] `resize set N ppt` respeta ratio de hermanos
- [ ] Auto-balanceo: distribuir espacio equitativamente

#### 20.2 Resize Múltiple
- [ ] Resize afecta a hermanos adyacentes
- [ ] Redistribuir espacio al cerrar ventana
- [ ] Mantener ratios al añadir ventana

#### 20.3 Split con Ratio
- [ ] `split horizontal N%` - split con ratio inicial
- [ ] `split vertical N%`

#### 20.4 Balanceo
- [ ] `balance` - distribuir espacio equitativamente
- [ ] `balance horizontal/vertical`
- [ ] Auto-balance al cerrar ventana (opcional)

#### 20.5 Tamaños de Ventana
- [ ] Respetar min_width/min_height de ventana
- [ ] Respetar max_width/max_height
- [ ] Resize increments (para terminals)

#### 20.6 Animaciones
- [ ] Smooth resize animations
- [ ] Smooth move animations (ya existe parcialmente)
- [ ] Configurable animation speed

---

### **21. CONFIGURACIÓN ADICIONAL** (10 tareas)

#### 21.1 Focus Behavior
- [ ] `focus_follows_mouse yes/no` (ya existe, verificar)
- [ ] `mouse_warping output/none` - mover cursor al cambiar output
- [ ] `focus_wrapping yes/no/force/workspace` - wrap al navegar
- [ ] `focus_on_window_activation smart/urgent/focus/none`

#### 21.2 Popup Handling
- [ ] `popup_during_fullscreen smart` - mostrar popups importantes
- [ ] `popup_during_fullscreen ignore` - ignorar todos
- [ ] `popup_during_fullscreen leave_fullscreen` - salir de fullscreen

#### 21.3 Workspace Behavior
- [ ] `workspace_auto_back_and_forth yes/no`
- [ ] `force_focus_wrapping yes/no`

---

### **22. COMANDOS EXEC** (8 tareas)

#### 22.1 Exec Variants
- [ ] `exec <command>` - ejecutar al startup/reload
- [ ] `exec_always <command>` - ejecutar en cada reload
- [ ] `exec --no-startup-id <command>`

#### 22.2 Posicionamiento
- [ ] Ventana abierta por exec se posiciona según for_window
- [ ] Tracking de PID para associar con for_window

---

### **23. MISC FEATURES** (12 tareas)

#### 23.1 Comandos Varios
- [ ] `nop <comment>` - no operation (para comentarios en bindings)
- [ ] `debuglog toggle` - activar/desactivar debug log
- [ ] `shmlog <size>|toggle|on|off` - shared memory log

#### 23.2 Legacy Support
- [ ] `default_border normal|none|pixel` - alias de border
- [ ] `default_floating_border ...`
- [ ] Warnings para comandos deprecados

#### 23.3 Window Attributes
- [ ] `title_window_icon yes/no` - mostrar icon en title
- [ ] Window icon support (X11 _NET_WM_ICON)

---

### **24. CRITERIOS AVANZADOS** (10 tareas)

#### 24.1 Más Criterios
- [ ] `[id="..."]` - X11 window ID
- [ ] `[window_id="..."]` - igual que id
- [ ] `[machine="hostname"]` - ventanas remotas
- [ ] `[urgent="latest"]` - última ventana urgente
- [ ] `[floating_from="auto/user"]` - cómo se hizo floating

#### 24.2 Comparadores
- [ ] Regex matching: `title=".*pattern.*"`
- [ ] Exact match: `title="^exact$"`
- [ ] Case insensitive: `(?i)pattern`

---

### **25. CONFIGURACIÓN DE BAR AVANZADA** (15 tareas)

#### 25.1 Bar Position y Size
- [ ] `height <px>` - altura de bar
- [ ] `mode dock|hide|invisible` - comportamiento
- [ ] `hidden_state hide|show` - estado oculto
- [ ] `modifier Mod4` - tecla para mostrar bar oculta

#### 25.2 Workspace Buttons
- [ ] `workspace_buttons yes/no`
- [ ] `strip_workspace_numbers yes/no` - ocultar números
- [ ] `strip_workspace_name yes/no` - ocultar nombres
- [ ] `binding_mode_indicator yes/no`

#### 25.3 Tray
- [ ] `tray_padding <px>`
- [ ] `tray_output none|primary|<output>`

#### 25.4 Separators
- [ ] `separator_symbol <string>` - entre bloques status
- [ ] Custom separator rendering

---

### **26. BACKEND ESPECÍFICO** (8 tareas)

#### 26.1 X11 Backend (XWayland)
- [ ] Soporte completo XWayland (verificar estado)
- [ ] _NET_WM_* properties
- [ ] EWMH compliance completo
- [ ] Motif hints para CSD/SSD

#### 26.2 Wayland Native
- [ ] xdg-shell full support
- [ ] xdg-decoration protocol
- [ ] layer-shell (ya existe, verificar)

---

### **27. TESTING Y COMPATIBILIDAD** (10 tareas)

#### 27.1 Tests
- [ ] Property tests para layouts (ya existen parcialmente)
- [ ] Tests de IPC
- [ ] Tests de for_window rules
- [ ] Tests de workspaces

#### 27.2 Migración desde i3
- [ ] Script para convertir config i3 → niri
- [ ] Documentación de diferencias
- [ ] Lista de features no soportadas

#### 27.3 Compatibilidad
- [ ] i3-msg alias para niri msg
- [ ] Drop-in replacement para scripts
- [ ] IPC wire protocol 100% compatible

---

## 🎯 PRIORIDADES DE IMPLEMENTACIÓN

### **FASE 1: CORE FUNCTIONALITY** (Crítico - 4-6 semanas)
**Objetivo:** Funcionalidad básica equivalente a i3

1. **Workspaces Dinámicos** (20 tareas) - 1 semana
   - Desacoplar de monitores
   - Creación/destrucción automática
   - Nombres y números
   - Navegación completa

2. **Scratchpad** (12 tareas) - 1 semana
   - Workspace invisible
   - Move/show commands
   - Múltiples ventanas

3. **Marcas y Criterios** (15 tareas) - 1 semana
   - Sistema de marks completo
   - Criterios de selección avanzados
   - Navegación con marcas

4. **For_window y Assign** (20 tareas) - 1-2 semanas
   - Rules automáticas
   - Variables en config
   - Reload/restart

### **FASE 2: USABILIDAD** (Importante - 4-6 semanas)
**Objetivo:** Experiencia de usuario completa

5. **Resize Completo** (15 tareas) - 1 semana
   - Modo resize interactivo
   - Todos los comandos resize
   - Mouse support

6. **Borders Completos** (18 tareas) - 1 semana
   - Todos los estilos
   - Title bars completas
   - Smart borders/gaps

7. **Floating Mejorado** (18 tareas) - 1 semana
   - Sticky windows
   - Posicionamiento preciso
   - Memoria de posición

8. **Modos** (12 tareas) - 1 semana
   - Binding modes
   - Indicador visual
   - Modo passthrough

### **FASE 3: AVANZADO** (Nice to have - 6-8 semanas)
**Objetivo:** Feature parity completo con i3

9. **Bar Nativa** (15 tareas) - 2-3 semanas
   - Implementación completa
   - Workspace buttons
   - Status line
   - System tray

10. **IPC Completo** (20 tareas) - 2 semanas
    - Todos los comandos GET_*
    - Sistema de eventos completo
    - Wire protocol compatible

11. **Multi-monitor** (18 tareas) - 1-2 semanas
    - Workspaces independientes
    - Hot-plug robusto
    - Configuración completa

12. **Layouts Avanzados** (22 tareas) - 2 semanas
    - Toggle layouts
    - Auto-split
    - Tabbed/stacked completo

### **FASE 4: POLISH** (Refinamiento - 4-6 semanas)
**Objetivo:** Detalles y compatibilidad 100%

13. **Urgency** (8 tareas) - 1 semana
14. **Gaps Completo** (10 tareas) - 1 semana
15. **Títulos** (10 tareas) - 1 semana
16. **Tiling Avanzado** (15 tareas) - 1-2 semanas
17. **Misc Features** (12 tareas) - 1 semana
18. **Testing** (10 tareas) - 1 semana

---

## 📊 RESUMEN TOTAL

### Por Categoría
| # | Categoría | Tareas | Prioridad | Tiempo Est. |
|---|-----------|--------|-----------|-------------|
| 1 | Workspaces | 20 | 🔴 Alta | 1 sem |
| 2 | Movimiento/Focus | 25 | 🔴 Alta | 1-2 sem |
| 3 | Scratchpad | 12 | 🔴 Alta | 1 sem |
| 4 | Marcas/Criterios | 25 | 🔴 Alta | 1 sem |
| 5 | Layouts | 22 | 🟡 Media | 2 sem |
| 6 | Resize | 15 | 🟡 Media | 1 sem |
| 7 | Gaps | 10 | 🟢 Baja | 1 sem |
| 8 | Borders | 18 | 🟡 Media | 1 sem |
| 9 | Floating | 18 | 🟡 Media | 1 sem |
| 10 | Bar UI | 30 | 🟡 Media | 2-3 sem |
| 11 | Modos | 12 | 🟡 Media | 1 sem |
| 12 | Configuración | 20 | 🔴 Alta | 1-2 sem |
| 13 | Urgency | 8 | 🟢 Baja | 1 sem |
| 14 | Multi-monitor | 18 | 🟡 Media | 1-2 sem |
| 15 | IPC | 20 | 🟡 Media | 2 sem |
| 16 | Títulos | 10 | 🟢 Baja | 1 sem |
| 17 | Auto-layout | 8 | 🟢 Baja | 1 sem |
| 18 | Fullscreen | 10 | 🟡 Media | 1 sem |
| 19 | Inhibidores | 8 | 🟢 Baja | 1 sem |
| 20 | Tiling Avanzado | 15 | 🟢 Baja | 1-2 sem |
| 21 | Config Extra | 10 | 🟢 Baja | 1 sem |
| 22 | Exec | 8 | 🟡 Media | 1 sem |
| 23 | Misc | 12 | 🟢 Baja | 1 sem |
| 24 | Criterios Avanzados | 10 | 🟡 Media | 1 sem |
| 25 | Bar Avanzada | 15 | 🟡 Media | 1-2 sem |
| 26 | Backend | 8 | 🟢 Baja | 1-2 sem |
| 27 | Testing | 10 | 🟡 Media | 1 sem |

### **TOTAL GENERAL**
- **Tareas totales:** ~370
- **Ya implementado:** ~40 (11%)
- **Pendiente:** ~330 (89%)
- **Tiempo estimado total:** 25-35 semanas (6-9 meses)

---

## 🔧 NOTAS TÉCNICAS

### Cambios Arquitectónicos Necesarios

1. **Workspace System Refactor**
   - Desacoplar completamente de Output
   - Workspace como entidad independiente
   - Manager de workspaces global
   - Asignación dinámica a outputs

2. **Marks System**
   - HashMap de marks → NodeKey
   - Persistencia en estado
   - Serialización para restart

3. **Scratchpad Implementation**
   - Workspace especial tipo "hidden"
   - Stack LIFO para múltiples ventanas
   - Posicionamiento centrado automático

4. **IPC Refactor**
   - Estructura de mensajes compatible con i3
   - Sistema de eventos con subscriptions
   - Socket Unix domain
   - Thread separado para IPC

5. **Bar Implementation**
   - Renderer independiente (Cairo/Pango)
   - Protocolo de comunicación con status command
   - SNI (StatusNotifierItem) para tray
   - Click event handling

6. **Modes System**
   - Stack de modos activos
   - Override de bindings por modo
   - Indicador visual en bar

7. **Criteria Matching Engine**
   - Parser de criterios regex
   - Evaluador eficiente
   - Cache de matches

8. **Layout Persistence**
   - Serialización completa del árbol
   - Restauración exacta en restart
   - Versioning de formato

### Compatibilidad con i3/sway

**Wire Protocol:**
- IPC socket idéntico
- JSON schema compatible
- Message types iguales
- Event types iguales

**Config Syntax:**
- Parser compatible (subset)
- Variables con `set $var value`
- Includes funcionando
- Comments con `#`

**Herramientas:**
- `niri-msg` = `i3-msg` (symlink o alias)
- Scripts de i3 funcionan sin cambios
- i3status/waybar compatibles

### Performance Considerations

**SlotMap Optimization:**
- Ya implementado, mantener
- O(1) access por NodeKey
- Generational indices

**Render Pipeline:**
- Damage tracking preciso
- Occlusion culling
- Layer optimization

**Event Loop:**
- IPC en thread separado
- No bloquear main loop
- Async command execution

---

## 📚 REFERENCIAS

### Documentación i3/sway
- [i3 User's Guide](https://i3wm.org/docs/userguide.html)
- [i3 IPC Protocol](https://i3wm.org/docs/ipc.html)
- [sway(5) man page](https://man.archlinux.org/man/sway.5)
- [sway-ipc(7)](https://man.archlinux.org/man/sway-ipc.7)

### Implementación de Referencia
- [i3 source code](https://github.com/i3/i3)
- [sway source code](https://github.com/swaywm/sway)
- [wlroots](https://gitlab.freedesktop.org/wlroots/wlroots)

### Protocolos Wayland
- xdg-shell
- xdg-decoration
- layer-shell
- idle-inhibit
- xdg-activation

---

## ✅ CHECKLIST DE VERIFICACIÓN

Antes de considerar "feature complete":

- [ ] Todos los comandos de i3 implementados
- [ ] IPC 100% compatible
- [ ] Config de i3 funciona con mínimos cambios
- [ ] Scripts de i3 funcionan sin modificación
- [ ] i3-msg puede ser reemplazado por niri-msg
- [ ] Bar compatible con i3status/waybar
- [ ] Tests de regresión para todos los features
- [ ] Documentación completa de diferencias
- [ ] Script de migración i3 → niri
- [ ] Performance comparable o mejor que i3/sway

---

**Última actualización:** 2025-10-06
**Versión:** 1.0
**Estado:** Planificación completa
