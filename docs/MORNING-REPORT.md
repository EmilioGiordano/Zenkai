# Reporte de la mañana (2026-10-08)

## En una línea

Todo lo hecho está en la rama **`fix/overnight-sweep`** (`6b44895`): 159 commits lineales
sobre `main`. Cada rama intermedia es un punto de esa misma pila. No hubo push ni merge;
`main` solo recibe este reporte.

Cada tanda pasó por reviews de calidad, y de seguridad y rendimiento cuando correspondía,
hasta dar PASS. La validación final sobre la punta está toda en verde: `cargo fmt --check`,
`clippy -D warnings`, 138 tests, `cargo deny`, build release y prueba abriendo un archivo
real.

## Cómo probarlo

```powershell
git checkout fix/overnight-sweep
cargo build --release -p zenkai -j 6
.\target\release\zenkai.exe                 # libro vacío
.\target\release\zenkai.exe ruta\libro.xlsx # o .xls/.ods/.csv
```

Ctrl+Shift+P abre la paleta con todos los comandos. Los atajos son los de Excel para
Windows.

## Qué hay

- **Archivos.**
  - Abrir y guardar `.xlsx` con guardado atómico verificado.
  - `.xls`, `.ods`, `.xlsb` y `.xlsx` rotos se abren en solo lectura (calamine).
  - CSV con vista previa:
    - separador, codificación, coma decimal y fechas d/m/a, cada uno con selector;
    - la tabla muestra los valores tal como se importan.
  - Autoguardado y recuperación.
  - Archivos recientes en la paleta.
  - Soltar un archivo en la ventana lo abre. Es lo único que no probé en vivo: no se puede
    automatizar arrastrar desde el Explorador.
  - Si guardar falla, se abre Guardar como.
- **Grilla.**
  - Formatos de Excel, celdas combinadas, paneles inmovilizados, desborde de texto.
  - Barra de fórmulas editable (Enter confirma, Esc cancela).
  - La barra de estado nombra los modos ("Showing formulas", "Read-only").
  - Ajuste de texto y alineación vertical.
  - Números en formato General que no entran se acortan o pasan a notación científica.
  - Redimensionar columnas y filas; autoajuste de columnas.
  - Ocultar o mostrar filas y columnas (Ctrl+9 / Ctrl+0).
  - Controlador de relleno con series (números, fechas, "Item 1").
- **Edición.**
  - Fórmulas: modo punto con referencias coloreadas, autocompletado de funciones (Tab),
    F4 para alternar `$`.
  - Alt+Enter para salto de línea; Ctrl+Enter llena toda la selección.
  - Copiar y pegar con fórmulas; pegar valores (Ctrl+Shift+V).
  - Rellenar (Ctrl+D/R), Autosuma, Ctrl+; y Ctrl+Shift+;.
  - Buscar (Ctrl+F) y reemplazar (Ctrl+H).
  - Ordenar A→Z / Z→A y región actual (Ctrl+Shift+\*).
  - Mostrar fórmulas (Ctrl+\`).
  - F4 fuera de la edición repite el último formato.
  - Menú contextual en celdas y pestañas.
- **Formato.**
  - Negrita, cursiva, subrayado, tachado (Ctrl+5), tamaños de fuente (Ctrl+Shift+> / <).
  - Colores de fuente y relleno.
  - Bordes; alineación.
  - Formatos numéricos y Formato de celdas (Ctrl+1); aumentar o disminuir decimales.
  - Borrar formatos o todo.
- **Hojas.** Crear, renombrar, borrar, mover, duplicar; Ir a (Ctrl+G).
- **Vista.**
  - Tema claro, oscuro y alto contraste, que sigue al sistema.
  - Zoom; tamaño de interfaz separado (Ctrl+Alt+= / -); reducir movimiento.
  - Panel de diagnóstico (Ctrl+Shift+D).
  - Gráfico rápido de la selección (Alt+F1).
- **Accesibilidad.** Etiquetas AccessKit en la grilla, la celda activa, la barra de
  fórmulas y el cuadro de nombres.

El detalle está en `README.md`. Las decisiones, con su porqué, están en `DECISIONS.md`.

## Bugs reales encontrados esta noche (todos corregidos y con test)

- **IronCalc perdía al guardar** las filas vacías con altura, formato u ocultas. Ahora se
  reinsertan en el archivo guardado (`crates/engine/src/empty_rows.rs`).
  - Con un archivo hostil, IronCalc también escribía filas fuera de la grilla o con
    alturas imposibles, y Excel lo habría marcado como dañado. Ahora se descartan o se
    acotan, incluido el `<dimension>` que IronCalc derivaba de esas filas.
  - Conviene reportar ambas cosas upstream.
- **Atajos Ctrl+Shift+símbolo** (formatos Ctrl+Shift+$ % # ~ !) no funcionaban en Windows:
  GPUI reporta Shift+4 como "$" sin Shift. Verificado en vivo antes y después.
- **Cuelgues de borrado.**
  - Ctrl+A + Supr y Borrar todo hacían que IronCalc recorriera 17 mil millones de celdas.
  - Un Supr sobre una celda vacía lejana creaba celdas y agrandaba el área usada.
  - Formatear un rango parcial gigante también recorría todas sus celdas. Ahora se
    rechaza con un mensaje.
- **Seguridad: pánicos con datos de usuario.**
  - El autocompletado se caía con una letra no ASCII (`=é`) antes del cursor.
  - F4 desbordaba con nombres largos (`=INDIRECT`).
  - Las entradas `+…` y `-…` esquivaban los límites de fórmula.
  - El CSV no tenía tope de filas, columnas o celdas.
- **CSV.** La coma decimal convertía "0.123" en 123 (lo encontró una review antes de
  mergear).
- **Paneles.** Los clics atravesaban la vista previa de CSV y Formato de celdas y llegaban
  a la grilla.
- **Pegar valores** podía leer una hoja o un libro que había cambiado desde la copia. Ahora
  cualquier edición o cambio de documento termina el modo copia, como en Excel.
- **Abrir mientras se edita.** Si un archivo terminaba de abrirse después de que editaras
  el libro actual, lo reemplazaba sin volver a preguntar. Ahora pregunta, y una apertura
  más vieja nunca gana sobre una más nueva.
- **F2 durante un recálculo** abría el editor vacío, y Enter borraba la celda. Ahora
  avisa y espera.
- **Ahora hay tests de propiedad** que verifican que ninguna función que procesa texto o
  archivos entre en pánico con entradas arbitrarias, y que cualquier secuencia de
  ediciones se deshace y rehace exactamente.

## Decisiones que te tocan

1. **Merge.** `feat/core-app` (la base de toda la pila) quedó estacionada tras tres rondas
   de seguridad. Los arreglos están commiteados y las reviews de seguridad posteriores
   sobre la pila dieron PASS, incluido un barrido final de todo el motor y los formatos.
   Si te parece, mergeá `fix/overnight-sweep` entera. `main` ya tiene el commit de
   este reporte, así que la pila no entra por fast-forward: hace falta un merge commit o
   un rebase.
2. **Ctrl+0** ahora oculta columnas, como en Excel. Antes reseteaba el zoom, que sigue en
   la paleta.
3. **Presupuesto de edición < 50 ms.** IronCalc recalcula el libro entero en cada edición:
   fixture 2 tarda unos 260 ms y fixture 3 unos 160 s (era así desde la Fase 0). El resto
   del benchmark no tuvo regresiones (auditado tres veces esta noche).
4. **Pérdidas conocidas al guardar** (límite de IronCalc):
   - con aviso al abrir: gráficos, imágenes, tablas, hipervínculos, validación,
     comentarios, autofiltro, protección;
   - sin aviso (cosméticas): márgenes, configuración de página, encabezado/pie, color de
     pestaña, zoom.

   El detalle está en `docs/COMPATIBILITY.md` (en la rama) y en DECISIONS.md.

## Diferencias con Excel que quedan (en DECISIONS.md)

- Varios pasos de deshacer donde Excel usa uno, porque IronCalc no tiene escritura por
  lotes: Reemplazar todo, el alto de filas al ajustar texto, Ctrl+Enter y Borrar todo en
  filas o columnas enteras.
- Ordenar no mueve el formato ni detecta encabezados.
- El relleno no copia formato.
- En la barra de fórmulas no hay modo punto (hacer clic en celdas no inserta
  referencias), la barra no crece con contenido largo, y el autocompletado solo funciona
  en la celda.
- Las fechas generadas por el relleno se ven como yyyy-mm-dd.
- La copia de hoja se llama "Sheet1 (1)" (Excel usa "(2)").

## Para verificar en tu máquina

- Los atajos Ctrl+Shift+símbolo se registran como los reporta GPUI en Windows (`ctrl-$`).
  En macOS o Linux probablemente no disparen así. Si vas a compilar para otra plataforma,
  hay que sumar la forma `ctrl-shift-$`.
- Todas las pruebas de atajos corrieron con el teclado en inglés (EE. UU.) activo, pero
  también tenés instalada la distribución española (Latinoamérica). Conviene probar
  Ctrl+;, Ctrl+Shift+$ y Ctrl+\` con la tuya.

## Limpieza

- No quedan procesos cargo, rustc ni zenkai.
- Borré los archivos de prueba que creé en `%LOCALAPPDATA%\Zenkai`.
- Quedan dos worktrees de review: `../zenkai-wt-review` (13 GB) y `../zenkai-wt-review2`
  (9 GB), casi todo compilaciones. Se borran con:

  ```powershell
  git worktree remove --force ..\zenkai-wt-review
  git worktree remove --force ..\zenkai-wt-review2
  ```
