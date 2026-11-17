Ocean Foundry - development build
=================================

This is a working copy for trying things out, not a finished release. Run
OceanFoundry.exe; nothing needs to be installed. BUILD.txt says which build
this is.

What works in this build
------------------------
  - Ocean: change the settings on the left and the sea is worked out again
    and shown as the blocks it will be built from. With "Live" on this
    happens by itself; otherwise press Bake (Ctrl+B).
  - Land, rivers and waterfalls (bottom of the left panel): choose Open sea,
    Coast or Cliffs. These are test lands; "Build file" brings your own
    build in as the land instead. "Land shape" gives another test land of
    the same kind.
      - Drag the dots on the small map to place a river: white is where it
        rises, coloured where it is headed. "Add a river" adds one. It runs
        downhill, falls where the ground drops away and ends at the sea.
      - "Delta" is how much sand the river has laid down at its mouth. A
        delta is only built where the shore has room for one: a low beach
        with shallow water in front. Under each river the card says how it
        ends (delta, plain mouth, waterfall, ...).
      - The coloured line along the shore on the map is how the shore was
        read from the shape of the land: beach, beach with room for a delta,
        rocky shore, cliff, river channel. The waves act accordingly: they
        run up a beach, break up among rocks, are thrown back by a cliff,
        and steepen against a river running out.
      - "Add a pond" puts still water in a hollow: drag its square on the
        map. "Depth" is how deep it stands over the lowest point; "Dig out"
        digs a round basin of that radius so it holds even on a slope (0
        fills only a hollow the land has). Where the hollow is too shallow
        the card says where the water runs out and how deep it stayed.
  - "Add calm water" and "Add a keep-dry area" work on every land,
    open sea too: drag the circle's dot on the map. Calm water has smaller
    waves and less foam, easing back to the open sea at the circle's
    edge; a keep-dry area has no water at all, to leave room for a build.
    The scene is kept in OceanFoundry-data\ocean-scene.json.
  - The view: drag to turn, right-drag to move, scroll to zoom, double-click
    to reset. The buttons at its top left hide the glass or show only the
    foundation.
  - Export: saves the baked ocean as Litematica (.litematic), WorldEdit
    (.schem) or Axiom (.bp) for Minecraft Java 26.1.2. The window says at
    which Y to paste it. "Only part of the plot" saves an area: type its
    edges or drag across the small map.
  - Files...: reads build files and converts them between the formats.

Not in this build yet: finding the rivers already in a brought-in build,
and the Lava mode.

Adjusting the ocean method
--------------------------
The generator's internal numbers are in

    OceanFoundry-data\ocean-tuning.json

(Advanced > Generator tuning > Open tuning file). Change a number, save, and
the sea is baked again. TUNING.txt explains every number. A mistake in the
file is reported with its line, and the previous build stays on screen.

The file carries "_version". When the method changes so much that old numbers
no longer mean the same, the app sets your file aside as
ocean-tuning.previous.json and writes a fresh one. When a newer build only
adds a number, your file simply does not have it yet and the default is used;
"Restore defaults" writes a complete file.

Test runs and the command line
------------------------------
test-runs\ holds pictures of every scene (open sea, coast, cliffs) in a calm,
a moderate and a stormy sea, with the build files. To make them again:

    OceanFoundry-cli testruns test-runs

One scene with your own numbers:

    OceanFoundry-cli ocean <folder> <name> [coast|cliffs] [tuning=FILE]
           [stream=x1,z1,x2,z2[,width[,winding[,delta]]] ...] [streams=none]
           [pond=x,z[,depth[,dig]] ...] [calm=x,z[,radius[,strength]] ...]
           [dry=x,z[,radius] ...]
           [ocean.<id>=value ...]

  - stream= places a river from (x1,z1) toward (x2,z2), as shares of the plot
    from its north-west corner; streams=none drops the land's own river.
  - pond= puts a pond at (x,z), as shares of the plot, with the depth and
    the radius dug in blocks (default 3 and 8). It prints how each came out.
  - calm= and dry= put a calm-water or keep-dry circle at (x,z), as shares
    of the plot, with its radius in blocks (default 16) and, for calm
    water, how much of the waves it takes away, 0 to 1 (default 0.8).
  - It also writes <name>-target.png: on the left the colours the method
    aims for, on the right the blocks chosen for them, both from above.
    Compare the two to tell a fault of the colour method from a limit of
    the block palette.
  - It writes <name>-view.png, <name>-map.png and, with land,
    <name>-shore.png: the shore as it was read. Yellow is beach, green beach
    with room for a delta, brown rocky shore, red cliff, turquoise running
    water and river channels. It also prints how each river ended.
  - "OceanFoundry-cli params" lists the ocean.* setting ids.

The app can be run without anyone at it, for screenshots:

    OceanFoundry.exe --data-dir <dir> --mode ocean --land coast|cliffs|open
        --bake --scroll <pixels> --lang en|zh-CN --appearance light|dark
        --size 1400x900 --screenshot <png>

More options for test runs:
    --set ocean.id=value          change one setting (repeatable)
    --pond x,z[,depth[,dig]]      add a pond to the land (repeatable)
    --calm x,z[,radius[,strength]]  add calm water (repeatable)
    --dry x,z[,radius]            add a keep-dry area (repeatable)
    --build <file> --waterline <0..1>   a build file as the land
    --show build|wanted|blocks|difference|height|depth|foam|normals|reflection
    --layers all|no_glass|foundation|surface|glass  --cut-open <0..1>
    --open-project <file>  --load-profile <name>  --restore-history <n>
    --save-project <file>  --save-profile <name>  (saved once the bake is done)
    --exclude <block> (repeatable)  --restore-blocks
    --pattern-image <png|jpg>     the ripples' image (a saved project
                                  carries a copy; it is unpacked into
                                  OceanFoundry-data\maps when the original
                                  is missing)
    --export-to <file> --files-format litematic|sponge|axiom
        --export-selection complete|water_only|foundation|surface|all_glass|glass_1|glass_2|glass_3
        --export-area W,N,E,S --files-omit-air
        (the result is printed; without --screenshot the program closes
        once the file is saved, with exit code 1 if it could not be)

The command-line tool takes the same ideas: project=<file>, exclude=a,b,
area=W,N,E,S and pattern_image=<png> after "ocean", and
"OceanFoundry-cli project <file.ofproject> <folder>" bakes a saved project
exactly as the program would.

Getting a newer build
---------------------
Close the program, then run scripts\dev-build.ps1 in the project folder. It
replaces the program files here and leaves OceanFoundry-data - your settings,
your scene and your tuning - alone. To start afresh, delete OceanFoundry-data.


Ocean Foundry 海洋铸造厂 - 开发版
================================

这是用于试用的开发版，不是正式发布版。运行 OceanFoundry.exe 即可，无需安装。

  - 海洋：修改左侧设置，海洋会重新计算并以实际方块显示。打开"实时"会自动
    烘焙，否则按"烘焙"（Ctrl+B）。
  - 陆地、河流与瀑布（左侧面板底部）：选择开阔海面、海岸或悬崖。这些是测试用
    的陆地；"建筑文件"可以把你自己的建筑作为陆地导入。在小地图上拖动圆点放置河流；
    "三角洲"决定河口堆积的沙量，只有低平沙滩、前方浅水的海岸才会形成三角洲。
    地图上沿岸的彩色线是根据陆地形状判读的海岸类型（沙滩、可形成三角洲的
    沙滩、岩石海岸、悬崖、河道），波浪随之表现不同。"添加池塘"在洼地中放入
    静水：在地图上拖动它的方块；"深度"是水面高出最低点的格数，"挖出半径"
    挖出圆形池底，使斜坡上也能蓄水。"添加平静水域"和"添加干燥区域"适用于
    所有陆地（包括开阔海面）：在地图上拖动圆圈的圆点。平静水域浪更小、泡沫
    更少，到圆圈边缘渐渐回到外海；干燥区域完全不放水，为建筑留出空间。
  - 视图：拖动旋转，右键拖动平移，滚轮缩放，双击复位。
  - 导出：保存为 Litematica、WorldEdit 或 Axiom 文件（Minecraft Java 26.1.2），
    窗口中会说明粘贴的 Y 高度。"仅导出部分地块"可以输入边界或在小地图上拖动框选。

尚未包含：识别导入建筑中已有的河流，以及熔岩模式。

生成器的内部数值在 OceanFoundry-data\ocean-tuning.json 中，修改并保存后会
重新烘焙；TUNING.txt 解释了每个数值。test-runs 文件夹中有各场景在平静、
中等和风暴海况下的图片。

更新版本：关闭程序，在项目文件夹中运行 scripts\dev-build.ps1。它只替换程序
文件，不会改动 OceanFoundry-data（你的设置、场景和调校）。
