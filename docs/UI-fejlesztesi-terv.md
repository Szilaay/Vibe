# VIBE – UI-fejlesztési terv

Az elosztás marad: bal oldali workspace-fa, középen több AI-panel, jobb oldali munkaterület. A következő változások a napi kódolást könnyítik meg.

## Elkészült ebben a körben: OpenRouter VIBE Agent

- Kompakt, a natív CLI-panelekhez igazított fejléc. Modell és szolgáltatóváltás a hárompontos menüben.
- Monospace munkafolyamat, jól olvasható 14 pontos válaszok, külön felhasználói prompt, összecsukható eszközeredmények, folyamatos válaszmegjelenítés.
- Projektmappák listázása, szövegfájlok olvasása sorszámokkal, tartalomkeresés.
- Fájllétrehozás és teljes szövegcsere előnézettel. Az eredeti és javasolt változat egymás mellett látszik, mindkettő másolható.
- Az írás külön jóváhagyást igényel, az elutasítás/bezárás/leállítás nem ír fájlt. Megváltozott eredetit nem ír felül.
- Helyi `/help`, `/model`, `/model szolgáltató/modell`, `/clear` parancsok hálózat nélkül.

Ez saját, az appba épített OpenRouter-ügynök; nem külső PTY CLI. A Codex és Antigravity továbbra is az eredeti termináljukat használják. Az OpenRouter-ügynökben egyelőre nincs shell, tesztfuttatás vagy fájltörlés.

## Elkészült a második körben: fájlböngésző, változások és csatolás

A korábbi Browser helyőrzőt a Files váltotta fel. Lusta mappafa, fájlelőnézet, betöltött fájlnevek szűrése és OpenRouter-promptcsatolás működik. A Changes külön Git-fület, helyi státuszokat és színezett index-/munkafájl-diffet kapott, kézi frissítéssel. A csatolt fájlok útvonalai a piszkozattal mentődnek; tartalmuk csak küldéskor kerül a modellhez.

- Jobb oldalt Files / Changes / Activity fülek.
- Files: lusta betöltésű mappafa, fájlnévszűrő, fájlelőnézet. Nagy mappánál is reszponzív marad.
- Fájlnál „Hozzáadás a prompthoz”; a csatolt fájlok külön chipként jelennek meg a bevitel felett, eltávolíthatók küldés előtt.
- Changes: valós Git-változások, színes soronkénti diff, fájlonkénti megnyitás. Megbízható mentés/visszavonás külön fejlesztésként.
- Activity: olvasott és módosított fájlok panelenként; kattintás a megfelelő előnézetre ugrik.

Elfogadási feltétel: a megjelenített fájl és változás mindig tényleges lemez/Git-adatból származik; nincs teljes projekt automatikus feltöltése.

## Következő 2: navigáció és gyors műveletek

Jelenlegi hiány: az Agent / Code / Thread fejléc csak a Code nézetet jelöli, nincs mögötte három külön nézet.

- Code: jelenlegi osztott paneles munkatér.
- Agent: minden futó feladat állapota, várakozó jóváhagyások, gyors leállítás.
- Thread: kereshető beszélgetéslista, archiválás és folytatás.
- Ctrl+K parancskereső: workspace-váltás, új panel, fájlkeresés, beállítások, frissítés.
- A panelek műveletei egységes helyen: menü, nagyítás, bezárás; az állapotok szöveggel is olvashatók.

Elfogadási feltétel: minden kattintható elem valódi műveletet hajt végre; billentyűzettel is végigjárható.

## Következő 3: bevitel és beszélgetéskezelés

- Bevitelnél pontos modellnév, fájlcsatolmányok, becsült költség egyértelműen becslésként jelölve.
- Ctrl+F keresés a panelben, találatok közötti léptetés.
- „Legfrissebb” gomb az OpenRouter-naplóhoz is, ha a felhasználó visszagörgetett.
- Válasz/művelet másolása egy kattintással; beszélgetés exportja.
- Modellválasztóban külön „tools támogatás” jelzés a szolgáltató tényleges modelladataiból.
- OpenRouter-kulcs opcionális mentése az operációs rendszer hitelesítéstárában. Jelenleg csak a munkamenet memóriájában él.

## Következő 4: egységes megjelenés és hozzáférhetőség

- Egységes magyar vagy angol felület; jelenleg keverednek a nyelvek.
- Állandó panelrádiusz, háttérszínek, fókuszszegélyek és menüszélességek.
- 36–40 pontos interakciós célok, részletes tooltip a ritkán használt ikonoknál.
- Rövid, 120–180 ms átmenetek, csökkentett mozgás beállítással. A terminál karakterrácsát nem animáljuk és nem méretezzük transzformációval.
- Kisebb ablakon a másodlagos információ menübe kerül; ne legyen egymásra lógó cím és panelgomb.

## Műszaki háttér

A nagy `desktop_ui.rs` és `desktop_controller.rs` fájlokat funkciónként érdemes bontani, fokozatosan: workspace-fa, account/usage, agent panel, projektfájlok, beállítások. A fájlműveletek és HTTP-kérések továbbra is háttérfeladatok legyenek. Új automatikus usage-pollingot ez a fejlesztés nem ad hozzá.

Javasolt következő csomag: Ctrl+K parancskereső, panelkeresés és beszélgetés-archívum. A fájlböngésző és a diff további lépése az Activity nézet és a jól látható változásszámláló.
