# Spec inicial: planilla liviana en GPUI

## Contexto y principios

Construir una planilla de escritorio nativa, liviana y open source que abra y edite archivos .xlsx, lo más parecida a Excel posible. Nombre del proyecto: Zenkai. El motor de cálculo no se escribe desde cero: se usa una librería existente en Rust (IronCalc o logisheets-rs, a decidir en la Fase 0). La interfaz se construye con GPUI.

Esta spec está escrita para un agente que va a implementar el proyecto de forma autónoma. Ante una decisión no cubierta acá, el agente elige la opción que más se parezca al comportamiento de Excel y la registra en `DECISIONS.md`.

Prioridades, en este orden. Cuando dos entren en conflicto, gana la de arriba:

1. **Fiabilidad:** nunca perder ni corromper datos del usuario. Un archivo que se abre y se guarda sin cambios debe quedar equivalente al original en todo lo que la app soporta, y advertir antes de perder lo que no soporta.
2. **Motor correcto:** los resultados de las fórmulas coinciden con los de Excel.
3. **Visualización fluida con GPUI:** la grilla se mueve a 60 fps incluso con archivos grandes.
4. **Accesibilidad:** todo se puede hacer con teclado, con los mismos atajos que Excel.
5. **Funcionalidad:** se agrega recién cuando lo anterior está sólido.

Principio de diseño: **no reinventar nada.** Atajos, nombres de menú, comportamiento de selección, mensajes de error de fórmulas (`#DIV/0!`, `#N/A`, etc.) y layout general copian a Excel. Un usuario de Excel tiene que poder usar la app sin aprender nada nuevo.

## Alcance de la v0.1

La v0.1 es una planilla que abre, muestra, edita, recalcula y guarda xlsx de uso cotidiano con total confianza. Plataforma principal: Windows 10/11; macOS y Linux deben compilar en CI desde el día uno.

Dentro de la v0.1:

- Abrir y guardar .xlsx; importar .csv.
- Varias hojas con pestañas (crear, renombrar, reordenar, borrar).
- Edición de celdas y barra de fórmulas.
- Recálculo automático con el conjunto mínimo de funciones (ver sección de funcionalidad).
- Mostrar fielmente el formato que trae el archivo: formatos numéricos, fuente, negrita, cursiva, colores, bordes, alineación, anchos de columna, altos de fila, celdas combinadas y paneles inmovilizados.
- Editar formato básico: negrita, cursiva, alineación, formato numérico (general, número, moneda, porcentaje, fecha).
- Selección, copiar, cortar y pegar compatible con Excel; deshacer y rehacer; buscar; zoom.

Fuera de la v0.1 (no implementar, aunque el motor lo permita):

- Gráficos, tablas dinámicas, macros y VBA, imágenes y formas.
- Edición de formato condicional, validación de datos y comentarios (si el archivo los trae, se preservan o se advierte, ver Fiabilidad).
- Colaboración, versión web, impresión, temas personalizados.
- Escritura de .xls y .ods (la capa de formatos debe permitir sumarlos sin tocar el resto).

## Compatibilidad de formatos

La meta a largo plazo es abrir y guardar fielmente cualquier xlsx, csv o formato de planilla compatible. La fidelidad total es progresiva, pero desde la Fase 1 la arquitectura y las pruebas se construyen para ella, y en la v0.1 ningún archivo válido puede colgar la app ni perder datos en silencio.

Requisitos desde la Fase 1:

- **Capa de formatos separada del motor:** cada formato es un adaptador de lectura y escritura detrás de una interfaz común. En la v0.1: xlsx (a través del motor), csv y tsv (propios). Sumar xlsm, xls u ods después no debe tocar la grilla ni el motor.
- **Lectura tolerante:** aceptar las desviaciones menores del estándar que Excel acepta (archivos generados por librerías, OOXML transitional y strict, strings compartidos o en línea, sistema de fechas 1904).
- **Plan B de solo lectura:** si el motor no puede abrir un archivo, intentar abrir sus valores con `calamine` (lee xlsx, xls y ods) en modo solo lectura, con un aviso claro, en vez de fallar.
- **CSV como Excel:** detectar separador (coma, punto y coma, tabulación, barra), codificación (UTF-8 con y sin BOM, UTF-16, Windows-1252), campos entre comillas con saltos de línea, y decimales y fechas según configuración regional, con una vista previa antes de importar. Exportar en UTF-8 con BOM por defecto para que Excel lo abra bien.

Corpus de compatibilidad:

- `fixtures/compat/` reúne archivos de distintos orígenes: Excel en Windows y Mac, Google Sheets, LibreOffice, Numbers, y archivos generados por librerías (openpyxl, XlsxWriter, PhpSpreadsheet, Apache POI), incluyendo casos con Unicode, hojas muy anchas o largas y archivos con contenido no soportado.
- Una prueba recorre el corpus y genera `docs/COMPATIBILITY.md`: por archivo, si abre, si los valores coinciden con los cacheados por Excel, si sobrevive la ida y vuelta y qué se perdería al guardar. Cada bug de compatibilidad reportado suma su archivo al corpus.

## Fase 0: evaluación de motores

Antes de escribir una línea de interfaz, el agente construye un benchmark en Rust que compara [IronCalc](https://github.com/ironcalc/IronCalc) y [logisheets-rs](https://users.rust-lang.org/t/announcing-logisheets-rs-a-rust-spreadsheet-engine-with-xlsx-support/141121) y entrega un informe con una recomendación. Sin este informe no se avanza a la Fase 1.

**Archivos de prueba.** Un generador produce xlsx sintéticos reproducibles (semilla fija), más una carpeta `fixtures/real/` donde el usuario puede dejar archivos propios:

1. Solo valores: 100.000 filas × 20 columnas (números, textos y fechas mezclados).
2. Fórmulas simples: 50.000 filas con SUM, IF y aritmética por fila.
3. Fórmulas pesadas: 20.000 filas con VLOOKUP, XLOOKUP, SUMIFS y COUNTIFS contra una tabla de 10.000 filas en otra hoja.
4. Cadena de dependencias: 10.000 celdas donde cada una depende de la anterior.
5. Formato: 10.000 filas con estilos variados, celdas combinadas y anchos de columna.

**Métricas por motor y archivo** (en release, Windows como plataforma de referencia, 5 corridas, mediana):

| Métrica | Cómo se mide |
| --- | --- |
| Memoria pico al abrir (MB) | Pico de memoria del proceso durante la carga |
| Memoria en reposo (MB) | Memoria del proceso 5 s después de cargar |
| Tiempo de apertura (ms) | Desde la llamada de carga hasta el modelo listo |
| Recálculo completo (ms) | Reevaluar todo el libro |
| Recálculo tras editar una celda (ms) | Cambiar una celda raíz y obtener los dependientes |
| Tiempo de guardado (ms) | Escribir el xlsx a disco |
| Fidelidad de ida y vuelta | Abrir, guardar y reabrir: comparar valores, fórmulas y estilos celda por celda |
| Corrección contra Excel | Comparar valores recalculados contra los valores cacheados que Excel guarda en el archivo |
| Cobertura de funciones | Cuáles del conjunto mínimo están soportadas |

Como referencia, el informe incluye la memoria de Excel (o LibreOffice si Excel no está disponible) abriendo los mismos archivos, medida a mano por el usuario si hace falta.

**Entregable:** `docs/BENCHMARK.md` con las tablas de resultados, el código del benchmark en `bench/` reproducible con un solo comando, y una recomendación justificada. Criterios de decisión, en orden: corrección contra Excel, fidelidad de ida y vuelta, memoria, cobertura de funciones, velocidad, calidad y estabilidad de la API, actividad del proyecto y licencia compatible con Apache-2.0 o MIT.

Si ningún motor alcanza un nivel aceptable de corrección o fidelidad, el agente se detiene y lo reporta en vez de forzar una elección.

## Arquitectura y stack

Rust en todo el proyecto, con el motor aislado detrás de una interfaz propia para poder cambiarlo sin tocar la UI. Licencia del proyecto: Apache-2.0 (verificar compatibilidad con el motor elegido).

| Pieza | Elección | Notas |
| --- | --- | --- |
| UI | GPUI + gpui-component | gpui-component para barra de fórmulas, pestañas, menús y diálogos |
| Grilla | Elemento propio de GPUI | No usar la tabla de gpui-component como grilla principal |
| Motor | IronCalc o logisheets-rs | Según Fase 0 |
| Diálogos de archivo | crate `rfd` | Diálogos nativos |
| CSV | crate `csv` | Detección de separador y codificación |

Estructura del workspace de Cargo:

- `crates/engine`: el trait `Engine` (cargar, guardar, leer rango, escribir celda, deshacer, rehacer, hojas) y su implementación sobre el motor elegido. Es el único crate que conoce al motor.
- `crates/grid`: el elemento de grilla de GPUI. Dibuja solo las celdas visibles, maneja selección, scroll, encabezados, paneles inmovilizados y celdas combinadas.
- `crates/app`: ventana, barra de fórmulas, pestañas de hojas, menús, acciones y atajos.
- `bench/`: el benchmark de la Fase 0, mantenido para medir regresiones.

Reglas de arquitectura:

- El hilo de UI nunca bloquea: carga, guardado y recálculos que superen un frame corren en segundo plano, con indicador de progreso.
- La grilla lee de una caché del rango visible (valores formateados y estilos), no del motor en cada frame. El motor notifica qué celdas cambiaron y la caché se invalida solo para esas.
- Si el modelo del motor no puede moverse entre hilos, se encapsula en un hilo dedicado que recibe comandos por canal. Documentar la decisión en `DECISIONS.md`.
- El trabajo por frame es proporcional a lo visible, nunca al tamaño del archivo.

## Funcionalidad y funciones mínimas

La interfaz replica la de Excel: barra de fórmulas arriba con el cuadro de nombre a la izquierda (muestra la celda activa, por ejemplo `B7`, y permite escribir una dirección para saltar), grilla con encabezados de columna (A, B, C…) y fila (1, 2, 3…), pestañas de hojas abajo y barra de estado con suma, promedio y recuento de la selección.

Comportamiento de edición que debe ser idéntico a Excel:

- Escribir sobre una celda reemplaza su contenido; F2 o doble clic edita el contenido existente.
- Enter confirma y baja; Tab confirma y va a la derecha; Esc cancela.
- Al escribir una fórmula, hacer clic o moverse con flechas inserta referencias, y las referencias se colorean en la fórmula y en la grilla.
- Copiar y pegar usa texto separado por tabulaciones en el portapapeles, para intercambiar datos con Excel y Google Sheets en ambos sentidos. Pegar fórmulas ajusta las referencias relativas.
- Los errores se muestran con los códigos de Excel: `#DIV/0!`, `#N/A`, `#VALUE!`, `#REF!`, `#NAME?`, `#NUM!`.

Conjunto mínimo de funciones para la v0.1, elegido por ser el que cubre la gran mayoría de las planillas cotidianas:

| Categoría | Funciones |
| --- | --- |
| Matemáticas | SUM, SUMIF, SUMIFS, PRODUCT, ROUND, ROUNDUP, ROUNDDOWN, ABS, MOD, INT |
| Estadísticas | AVERAGE, AVERAGEIF, AVERAGEIFS, MIN, MAX, COUNT, COUNTA, COUNTBLANK, COUNTIF, COUNTIFS, MEDIAN |
| Lógicas | IF, IFS, AND, OR, NOT, IFERROR, IFNA |
| Búsqueda | VLOOKUP, HLOOKUP, XLOOKUP, INDEX, MATCH |
| Texto | CONCAT, CONCATENATE, TEXTJOIN, LEFT, RIGHT, MID, LEN, TRIM, UPPER, LOWER, PROPER, SUBSTITUTE, FIND, SEARCH, TEXT, VALUE |
| Fecha | TODAY, NOW, DATE, YEAR, MONTH, DAY, WEEKDAY, EDATE, EOMONTH, DATEDIF, NETWORKDAYS |
| Información | ISBLANK, ISNUMBER, ISTEXT, ISERROR |

Más los operadores: aritméticos, comparación, concatenación con `&`, rangos `A1:B10`, referencias absolutas `$A$1` y referencias a otras hojas `Hoja2!A1`.

Estas funciones las provee el motor; la app no implementa ninguna. Si el motor elegido no soporta alguna, el agente la lista en `docs/BENCHMARK.md` y en un issue, sin implementarla por su cuenta en la app. Una función no soportada que aparezca en un archivo nunca se borra: se conserva la fórmula y el último valor que guardó Excel, y la celda se marca como no recalculable.

## Fiabilidad e integridad de datos

La regla central: la app nunca destruye en silencio algo que el usuario tenía. Todo lo que pueda perderse se advierte antes, y en la duda se guarda como archivo nuevo.

Guardado seguro:

- Guardado atómico: escribir a un archivo temporal en la misma carpeta, verificar que se puede reabrir, y recién entonces reemplazar el original.
- Recuperación: guardado automático cada 60 s en una carpeta de recuperación; al reabrir tras un cierre inesperado, ofrecer recuperar.
- Si el archivo está abierto por otro programa o es de solo lectura, ofrecer guardar como, nunca fallar en silencio.

Contenido no soportado:

- Al abrir, detectar lo que el motor no conserva al guardar (gráficos, tablas dinámicas, macros, imágenes, formato condicional, validaciones, comentarios, nombres definidos, lo que corresponda según el motor). Verificarlo empíricamente en la Fase 0, no suponerlo.
- Si hay algo así, mostrar un aviso no bloqueante al abrir y, al guardar, un diálogo que lista lo que se perdería y ofrece “Guardar como” con un nombre nuevo como opción por defecto.
- Archivos .xlsm: abrir, advertir que las macros no se ejecutan, y no permitir sobrescribir el original.

Pruebas obligatorias:

- **Corrección contra Excel:** un archivo guardado por Excel trae el último valor calculado de cada fórmula. Recalcular con el motor y comparar contra esos valores es la suite de corrección automática principal. Tolerancia para decimales documentada.
- **Ida y vuelta:** abrir, guardar y reabrir todos los fixtures, comparando valores, fórmulas y estilos soportados celda por celda.
- **Robustez:** archivos corruptos, truncados, vacíos o enormes no deben colgar ni cerrar la app; se muestra un error claro.
- **Deshacer:** propiedad verificada con pruebas: cualquier secuencia de ediciones seguida del mismo número de deshacer deja el libro igual al inicial.
- Ningún `unwrap()` o `panic!` en caminos alcanzables por datos del usuario.

## Rendimiento y presupuestos

Los presupuestos son provisorios y se recalibran con los números reales de la Fase 0; la meta es usar una fracción de la memoria de Excel con los mismos archivos.

| Medida | Presupuesto inicial |
| --- | --- |
| App abierta con libro vacío | < 150 MB de memoria, < 1 s hasta la ventana usable |
| Archivo 1 de la Fase 0 (100.000 × 20 valores) | < 3 s de apertura |
| Scroll y navegación en cualquier archivo | < 16 ms por frame (60 fps) |
| Editar una celda con dependientes típicos | < 50 ms hasta ver los resultados |
| Escribir en una celda (latencia de tecla) | < 16 ms |

Instrumentación:

- Un panel de diagnóstico activable con un atajo (por ejemplo Ctrl+Shift+D) muestra memoria del proceso, tiempo por frame y tiempo del último recálculo.
- El benchmark de `bench/` corre en CI y falla si una métrica empeora más de un 15 % respecto de la línea base guardada.
- Medir siempre en release; nunca sacar conclusiones de rendimiento en debug.

## Accesibilidad y teclado

Todo lo que se hace con mouse se puede hacer con teclado, con los atajos de Excel para Windows. En macOS, Ctrl se mapea a Cmd donde Excel para Mac lo hace.

| Atajo | Acción |
| --- | --- |
| Flechas / Ctrl+flechas | Mover una celda / saltar al borde del bloque de datos |
| Shift + lo anterior | Extender la selección |
| Ctrl+Home / Ctrl+End | Ir a A1 / a la última celda usada |
| Ctrl+Space / Shift+Space | Seleccionar columna / fila |
| Ctrl+A | Seleccionar todo |
| F2 / Enter / Tab / Esc | Editar / confirmar y bajar / confirmar y derecha / cancelar |
| Delete | Borrar contenido |
| Ctrl+C / Ctrl+X / Ctrl+V | Copiar / cortar / pegar |
| Ctrl+Z / Ctrl+Y | Deshacer / rehacer |
| Ctrl+S / Ctrl+O / Ctrl+N | Guardar / abrir / nuevo |
| Ctrl+F | Buscar |
| Ctrl+B / Ctrl+I | Negrita / cursiva |
| Ctrl+PageUp / Ctrl+PageDown | Hoja anterior / siguiente |
| Ctrl+Shift+P | Paleta de comandos con todas las acciones |

Requisitos de accesibilidad:

- Foco siempre visible, en la grilla y en el resto de la interfaz.
- Nunca comunicar estado solo con color: errores, celdas no recalculables y avisos llevan ícono o texto.
- Tema claro, oscuro y de alto contraste; respetar la configuración del sistema.
- Zoom de la grilla (Ctrl+rueda y Ctrl+/-) y escala de interfaz independiente.
- Respetar la preferencia de reducir movimiento del sistema.
- Lector de pantalla: investigar el soporte de accesibilidad disponible en el ecosistema GPUI (por ejemplo AccessKit, que menciona gpui-kit) y exponer al menos la celda activa (dirección, valor, fórmula), la barra de fórmulas y las pestañas. Si no es viable en la v0.1, documentar la brecha y dejar la arquitectura preparada.

## Fase 6: agentes dentro de Zenkai

Los agentes de IA (Claude Code, Gemini CLI, Codex y cualquier cliente MCP) trabajan sobre el **documento abierto**, nunca sobre el archivo en disco: no se recarga nada, no se pierde trabajo sin guardar, y cada cambio pasa por el mismo camino de edición que el del usuario (recálculo, grilla en vivo, deshacer). Zenkai no incluye ningún modelo ni agente: lanza o atiende los que el usuario ya tiene instalados.

### Alcance

Esta fase avanza en este orden, cada paso útil por sí solo:

1. **Archivo de configuración** con esquema, recarga en caliente, secretos en el Administrador de credenciales de Windows y una página de Configuración que detecta los agentes instalados.
2. **Capa de herramientas** en Rust puro sobre el documento abierto, sin LLM y probada de forma determinista.
3. **Puente MCP**: Zenkai como servidor MCP local, para que un Claude Code externo (o cualquier cliente MCP) lea y edite el libro abierto en vivo.

4. **Pestañas, espacios y sesión** (diseño visual a definir con el usuario):
   - Un espacio es un grupo con nombre de archivos; cada archivo es un ítem o pestaña.
   - Barra lateral opcional (Ctrl+B en la propuesta del usuario; en Excel Ctrl+B es negrita, así que el atajo se decide con el diseño), con un estilo sobrio a la Waku.
   - Carga diferida: las pestañas restauradas son enlaces hasta que se activan.
   - Las pestañas inactivas y sin cambios se descargan cuando falta memoria.
   - `session.json` restaura espacios, pestañas, hoja activa, selección y scroll; los cambios sin guardar vuelven por la recuperación.
   - Atajos de navegador: Ctrl+Tab, Ctrl+W, Ctrl+Shift+T y Ctrl+P (en Excel, Ctrl+P es imprimir, que está fuera del alcance).
   - Más adelante, vista dividida para dos archivos lado a lado.
5. **Chat de agentes** sobre ACP (Agent Client Protocol).
6. **Menciones de contexto** (`[Ventas!A1:N200]`).
7. **Capa de revisión**: resaltar cambios del agente, aceptar o rechazar, paso de deshacer con nombre.
8. **Generar datos** (diseño aprobado: los artboards Generate* de `spaces-mock/project/` y el artifact claude.ai/artifact/XYUMggmNUzgnraYvWceNNh):
   - Entradas: clic derecho sobre una selección, la paleta de comandos y Ctrl+Alt+G (configurable).
   - La selección define las columnas: solo encabezados, se agregan N filas debajo; encabezados y filas, se llenan las filas seleccionadas; una sola celda, se detecta la región alrededor; columnas vacías, el usuario las nombra. El rango es editable en el diálogo.
   - Por columna: encabezado editable (vacío o no) y tipo detectado del texto del encabezado. Las opciones de cada tipo se abren en un popover (por ejemplo, email: formato, columnas de origen, dominios y quitar acentos). Cada columna tiene además un % de vacíos y una marca de únicos.
   - Ajustes globales: cantidad de filas, ubicación, configuración regional de los datos (por ejemplo es-AR) y una semilla para reproducir el resultado.
   - Vista previa en vivo.
   - Nada se escribe hasta confirmar: Generar aplica los encabezados renombrados y las filas en un solo paso de deshacer; "Guardar solo encabezados" aparece cuando cambió un encabezado; Cancelar deja la hoja intacta; una columna renombrada muestra "Encabezado sin guardar" con la acción Descartar.
   - Solo local: sin IA y sin red.
   - Gancho para IA a futuro: la configuración de generación es un tipo serializable con JSON Schema. Una herramienta de agente puede producir la configuración, no los datos, lo que ahorra tokens al usuario, y el generador local produce las filas. Cuando conviene, el agente puede generar los datos él mismo.

Los pasos 4 a 8 se aprueban por separado. Las herramientas identifican el libro con un `WorkbookId` tipado desde el primer día, para que su forma no cambie cuando lleguen las pestañas. El resto de las ideas (Ctrl+K, auditoría, carpeta como espacio de trabajo) sigue en `docs/IDEAS.md`.

### Arquitectura

- `crates/agent` (`zenkai-agent`, biblioteca): configuración, capa de herramientas y puente MCP. Depende de `engine` y `types`, nunca de GPUI, para poder probarse sin ventana. Errores con `thiserror`.
- `crates/mcp-relay` (binario `zenkai-mcp`): relé mínimo, con `std` y solo el cliente de pipes de `interprocess` (un handle síncrono de `std` serializa lectura y escritura simultáneas), que copia stdin al pipe y el pipe a stdout. No tiene lógica MCP. Lo lanzan los agentes como servidor MCP por stdio, el único transporte que todos soportan.
- `crates/app`: une la capa de herramientas con `Document` y `Workspace::edit`, y dibuja la página de Configuración y la aprobación de escrituras.
- El servidor MCP usa `rmcp`, que necesita tokio: corre en un único hilo dedicado con un runtime `current_thread`. El hilo de UI nunca espera a tokio.
- Flujo de una llamada: el agente → `zenkai-mcp` → named pipe → `rmcp` en el hilo de tokio → solicitud tipada por un canal → bucle de `Workspace` en el hilo de UI → lectura en segundo plano o escritura por `Workspace::edit` → respuesta tipada por el mismo canal.
- Cada llamada lleva la generación del documento; si el usuario abrió o creó otro libro mientras tanto, la llamada se rechaza.
- Dependencias nuevas fijadas: `rmcp`, `interprocess`, `keyring-core` y `windows-native-keyring-store`. `tokio`, `notify`, `which` y `schemars` ya estaban en el árbol por GPUI.

### Configuración

- Archivo: `%APPDATA%\Zenkai\settings.json` (configuración del usuario, que viaja con el perfil). Cachés y estado siguen en `%LOCALAPPDATA%\Zenkai`. Fuera de Windows: `$XDG_CONFIG_HOME/zenkai` o `~/.config/zenkai`.
- Junto al archivo, Zenkai escribe `settings.schema.json`, generado con `schemars` desde los mismos tipos de Rust, para que un agente externo pueda editarlo sin equivocarse. El archivo lo referencia con `"$schema"`.
- Contenido: agente por defecto, agentes configurados (comando, argumentos, variables de entorno), modo de permisos (`read_only`, `ask_before_write` por defecto, `automatic`) y "Permitir agentes externos" (apagado por defecto).
- Recarga en caliente: se vigila la carpeta (los editores y agentes escriben por renombre), con espera de unos 200 ms; si el archivo nuevo no se puede leer, se mantiene la última configuración válida y se muestra un aviso no bloqueante con línea y columna. También se relee al volver el foco a la ventana.
- Secretos: nunca en el JSON. El JSON solo guarda una referencia `{ "secret": "<nombre>" }`; el valor vive en el Administrador de credenciales de Windows y el usuario lo carga desde la interfaz. Nunca se escribe en el log.
- Página de Configuración (acción con atajo y en la paleta de comandos): detección de `node`, `npx`, `claude` y `gemini` en el PATH, versión de Node, presets de Claude, Gemini y Codex con los comandos del registro de ACP, agente por defecto, modo de permisos y el interruptor de agentes externos. Los agentes por `npx` necesitan Node; si falta, Zenkai lo dice y explica cómo instalarlo.

### Herramientas

`list_workbooks`, `list_sheets`, `get_selection`, `read_range`, `find`, `write_cells`, `set_formula` y `format_range`. Ninguna guarda, ninguna expone la ruta del archivo ni el sistema de archivos.

- `list_workbooks` devuelve un `WorkbookId` por libro abierto, y cada herramienta de lectura o escritura recibe uno. Hoy hay un solo documento abierto; un id desconocido o de un libro ya reemplazado es un error tipado.

- `read_range` está paginada y limitada en celdas por llamada.
- `write_cells` y `set_formula` escriben **bloques rectangulares** con un tope de celdas por llamada, a través de `Workspace::edit`: recalculan, se ven en la grilla al instante y se deshacen con Ctrl+Z en un solo paso. Las escrituras dispersas quedan fuera hasta que el motor tenga deshacer por lotes.
- `format_range` respeta los mismos topes de formato que la interfaz.
- Una escritura se rechaza con un error claro si el libro es de solo lectura, si la sesión del agente es de solo lectura, si el usuario está editando una celda, o si la generación del documento cambió.

### Modelo de seguridad

La amenaza principal es la misma que en el resto de Zenkai, un archivo malicioso, ahora como **inyección de instrucciones**: celdas con texto escrito para que un agente lo obedezca. Las defensas:

- **Mínimo privilegio.** El agente solo ve las herramientas de arriba, limitadas al libro abierto: sin sistema de archivos, sin terminal, sin red, sin guardar. Los agentes que Zenkai lanza corren en una carpeta temporal vacía, sin capacidades `fs` ni `terminal` anunciadas.
- **Datos no confiables marcados.** Todo resultado que devuelve contenido de celdas lo encierra en bloques delimitados y etiquetados como "datos de planilla no confiables", con un marcador aleatorio por llamada que una celda no puede falsificar, y el texto de las celdas codificado como JSON. La descripción de cada herramienta dice que ese contenido es dato, nunca instrucciones.
- **Humano en el bucle.** Por defecto cada escritura pide aprobación (Permitir o Denegar, con teclado). Las escrituras tienen tope de celdas, nada se guarda solo y todo se deshace.
- **Vista protegida como Excel.** Si el archivo trae la Marca de la Web (el flujo alternativo `Zone.Identifier` con `ZoneId` 3 o 4), la sesión del agente empieza en solo lectura y la interfaz explica por qué. Se lee con `std::fs` sobre `ruta:Zone.Identifier`, en segundo plano al abrir.
- **Contenido oculto a la vista.** Cuando una herramienta lee de hojas, filas o columnas ocultas, o celdas de texto muy largo, el resultado lo dice, para que el usuario pueda verlo.
- **Canal local cerrado.** El pipe tiene un nombre aleatorio por sesión, rechaza clientes remotos, solo admite al usuario actual y exige un token aleatorio de 256 bits como primera línea, comparado en tiempo constante. Solo existe mientras "Permitir agentes externos" está encendido o hay una sesión del chat abierta.
- **Procesos.** Al cerrar una sesión se termina todo el árbol de procesos del agente con `taskkill /T`, sin código `unsafe`.
- **Configuración como interruptor.** Cualquier programa del usuario puede escribir `settings.json`. Un cambio que da más poder a los agentes (escribir sin preguntar, permitir agentes externos) no se aplica hasta que el usuario lo confirma en Zenkai, también al iniciar; quitar poder se aplica en el acto.
- **Comandos de agentes desde el archivo.** Antes de que el chat lance un agente, todo cambio en `agents.servers.*.command` o `args` que llegue desde el archivo (no desde la página de Configuración) se muestra y requiere confirmación. Zenkai nunca lanza agentes a través de shims `.cmd` o `.bat`: ejecuta `node` y el punto de entrada del paquete directamente, o valida cada argumento.
- **Riesgo residual.** Zenkai no puede apagar las herramientas propias de un agente (por ejemplo la terminal de Claude Code). La interfaz lo dice.

### Plan de ramas

| Rama | Contenido |
| --- | --- |
| `chore/phase-6-spec` | Esta sección y las decisiones |
| `feat/settings-file` | Tipos de configuración, esquema, recarga en caliente, secretos, página de Configuración con detección de agentes |
| `feat/workbook-tools` | Capa de herramientas tipada sobre el documento abierto, canal hacia el hilo de UI, aprobación de escrituras, Vista protegida y avisos de contenido oculto, pruebas deterministas |
| `feat/mcp-bridge` | `rmcp` en un hilo de tokio, named pipe restringido, relé `zenkai-mcp`, comando `claude mcp add` en Configuración, prueba de punta a punta |
| Pestañas, espacios y sesión | Paso 4; diseño visual a definir |
| `feat/agent-chat` y siguientes | Pasos 5 a 7: chat ACP, menciones y capa de revisión; se aprueban por separado |
| Generar datos | Paso 8: núcleo del generador en un crate de Rust puro; el diálogo llega después de las pestañas |

## Proceso de trabajo para el agente

El trabajo avanza por fases. Al cerrar cada una, el agente se detiene, entrega un resumen con lo hecho, lo pendiente y los riesgos, y espera aprobación antes de seguir.

1. **Fase 0, motor:** benchmark e informe de la sección anterior. Parada obligatoria con recomendación.
2. **Fase 1, núcleo sin UI:** crate `engine` con el trait, implementación sobre el motor elegido, capa de formatos con csv y plan B de solo lectura, corpus de compatibilidad, guardado atómico, detección de contenido no soportado y las suites de corrección, ida y vuelta y robustez pasando.
3. **Fase 2, grilla de solo lectura:** abrir un xlsx y mostrarlo con formato, scroll fluido, selección y navegación por teclado, pestañas de hojas. Cumplir el presupuesto de frame con el archivo 1.
4. **Fase 3, edición:** barra de fórmulas, edición de celdas, recálculo, copiar y pegar, deshacer, guardado y recuperación.
5. **Fase 4, terminaciones de la v0.1:** formato básico, buscar, zoom, temas, paleta de comandos, accesibilidad y empaquetado para Windows (instalador y ZIP portable).
6. **Fase 6, agentes dentro de Zenkai:** configuración, herramientas sobre el documento abierto y puente MCP, según su sección. Fuera de la v0.1.

Reglas durante todo el proyecto:

- No agregar funcionalidad fuera del alcance de esta spec, aunque sea fácil. Proponerla en `docs/IDEAS.md`.
- Cada decisión no cubierta acá se registra en `DECISIONS.md` con fecha, opciones consideradas y motivo.
- Antes de dar una fase por terminada: `cargo fmt`, `cargo clippy` sin advertencias, todas las pruebas en verde y el benchmark sin regresiones.
- Si una limitación del motor o de GPUI impide cumplir un requisito, detenerse y reportarlo con alternativas, en vez de esquivarlo en silencio.
- Mantener un `README.md` con cómo compilar, correr las pruebas y correr el benchmark en Windows.
