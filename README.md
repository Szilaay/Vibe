# VIBE

Nyílt forráskódú Rust asztali workspace több Codex / Antigravity CLI-hez és OpenRouter-ügynökhöz. A VIBE saját forrása [MIT licencű](LICENSE); a vendorizált függőség és a szolgáltatói assetek eredeti licencei saját könyvtárukban találhatók.

**Windows-telepítő:** [legfrissebb kiadás](https://github.com/Szilaay/Vibe/releases/latest). A telepítő és az aláírt frissítési fájlok továbbra is a Releases oldalon érhetők el.

## Fejlesztés forrásból

Rust stable (legalább 1.88) szükséges. Windows alatt Visual Studio C++ Build Tools és Windows SDK; Linux alatt a rendszerfüggőségeket a [CI konfigurációja](.github/workflows/ci.yml) sorolja fel.

```sh
git clone https://github.com/Szilaay/Vibe.git
cd Vibe
cargo build --locked --bin vibe
cargo run --locked --bin vibe
```

Ellenőrzés: `cargo fmt --all --check`, `cargo clippy --locked --all-targets -- -D warnings`, `cargo test --locked --all-targets`.

Az alap alkalmazás forrásból beszédmodell nélkül is fordítható; helyi diktáláshoz Windows alatt futtasd a `scripts/package-speech.ps1` scriptet. A nagy modellek, kész binárisok, helyi fiókok/mentések és privát frissítés-aláíró kulcsok nem részei a repónak. Saját fork kiadásához saját kiadási cél és saját aláírókulcs szükséges; a hivatalos frissítések nyilvános ellenőrzőkulcsa a forrás része.

### Kijelölési terület korlátozása (0.2.7)

Normál egérhúzás csak a natív CLI felismert beviteli promptjában kezdhet kijelölést, és a prompt végén megáll. Üres terminálterület és sessionválasztó nem jelölhető ki. A Codex sessionválasztójában a Ctrl+A továbbra is a natív archiválási parancs. Korábbi kimenet másolásához **Shift + egérhúzás**, majd **Ctrl+C** használható; ez csak olvasható kijelölés, Backspace/Delete nem módosítja. A másolási kijelölés nem festi be az üres sorokat és a jobb oldali üres margót.

### OpenRouter terminálnézet (0.2.6)

Az OpenRouter saját ügynöke a Codexhez illeszkedő, egységes monospace terminálnézetet kapott. A beviteli keret és a külön küldésgomb megszűnt: a közvetlen `›` prompt a beszélgetés után jelenik meg, alatta a modell és a projekt útvonala. A kódrészletek is keret nélküli kimenetként látszanak. A teljes tartalom közösen görgethető. Enter küld, Shift+Enter új sort szúr be; Ctrl+Z és a kijelöléses törlés megmaradt. Fájlcsatolás, eszközhívás-előzmények, módosításjóváhagyás és leállítás továbbra is elérhető. Ez az OpenRouter VIBE-ügynökének felülete; a natív Codex és Antigravity folyamatát nem cseréli le.

### Kijelölés javítása (0.2.5)

- A natív CLI egérkijelölése kurzorhatárokat használ: a kijelölés vége nem foglalja bele a következő karaktert, előre és hátrafelé húzásnál sem.
- Teljes sor kijelölésénél a promptjel és a jobb oldali üres hely nem számít törlendő szövegnek. Kimeneti vagy elavult kijelölés nem válik véletlen egykarakteres törléssé.
- A Ctrl+A-kijelölést nem szünteti meg az azonos bevitelen belüli késleltetett kurzorfrissítés. Ctrl melletti elnyelt szöveges esemény nem törli a kijelölést.
- A módosítás közös a Codex és Antigravity terminálnézetében. OpenRouterben a többsoros, fordított és Ctrl melletti kijelöléstörlés külön regressziós tesztet kapott.

### Workspace-szabályok és szövegszerkesztés (0.2.4)

- Workspace `…` / jobb kattintás: **Workspace szabályok**. Profilmenü: **Agent szabályok**, külön Globális és Workspace füllel.
- A globális VIBE-szabályok az alkalmazás adatkönyvtárának `rules.json` fájljába kerülnek. A workspace konfigurációja a kiválasztott projekt `.vibe/rules.json` fájljában tárolódik; ez csak kifejezett mentéskor jön létre.
- Prioritás: megadott helyi szabályfájl > workspace szöveg > globális VIBE-szöveg. A globális öröklés workspace-enként kikapcsolható. Helyi fájl: workspace-en belüli relatív UTF-8 útvonal, legfeljebb 8 KiB; összevont szabályok legfeljebb 12 KiB.
- OpenRouter a következő üzenetnél olvassa újra a szabályokat. Codex az induló folyamatnak átadott `developer_instructions` konfigurációt használja. Antigravity indításkor létrehozza/frissíti a VIBE által jelölt `.agents/rules/vibe-scoped.md` fájlt az adott workspace-ben, benne az érvényes globális és helyi szabályokkal. Ezt a generált fájlt ne szerkeszd kézzel; verziókezelés előtt ellenőrizd a tartalmát. Más eredetű azonos nevű fájlt nem ír felül.
- Natív CLI-nél szabálymódosítás után újraindítás szükséges. A VIBE-öröklés kapcsolója a VIBE saját szabályait kapcsolja ki; a külön telepített CLI `AGENTS.md`, `GEMINI.md`, profil- és szervezeti szabályait nem módosítja.
- Ctrl+Z: legfeljebb 10 korábbi szövegállapot, szerkesztőnként/panelenként külön kurzorral. A saját composer és szabályszerkesztő Unicode-kijelölést is visszaállít. Beküldés vagy külső draftcsere megszakítja a korábbi beviteli előzményt.
- A natív terminálban csak a felismert, látható prompt szerkesztését vonja vissza; nem a terminál kimenetét. Többsoros visszaállításhoz a CLI bracketed paste módja szükséges. Képernyőn kívülre görgetett, összecsukott vagy rejtett promptszöveg nem rekonstruálható megbízhatóan pusztán terminálcellákból.
- Backspace/Delete a bevitelen belüli egérkijelölést teljes egészében törli, a kurzort a tartomány elejére állítja. Kimeneti szöveget a kijelölés nem tesz szerkeszthetővé.

Natív integrációs leírások: [Codex konfiguráció](https://learn.chatgpt.com/docs/config-file/config-reference), [Antigravity workspace rules](https://www.antigravity.google/docs/rules/).

## CLI-jogosultságok megjegyzése

A VIBE nem kényszerít minden Codex-chatre új `workspace-write` / `on-request`
beállítást, és nem tiltja le külön az elmentett hálózati hozzáférést.
A `/permissions` menüben kiválasztott aktív módot a Codex natív hookja menti az
első üzenet elküldésekor, a válasz végén vagy megszakításkor. A következő Codex-panelek
és újraindítás utáni indítások ezt használják. A mód később ugyanott szigorítható is.
A mentett adat csak jogosultsági preset; nem tartalmaz beszélgetést vagy tokent.
A korábban megnyitott, változatlan jogosultságú panel nem írja felül az új választást.
A hookokhoz a Codex saját `/hooks` felületén egyszer jóvá kell hagyni a VIBE hookját.
Ha még nincs megjegyzett választás, a CLI saját tartós konfigurációja érvényesül.

Antigravity esetén a `/permissions` menü **Global** szabályai minden chathez
megmaradnak; a **Project** szabályok az adott projekthez tartoznak. A VIBE ugyanazt
a natív konfigurációt használja minden panelen, nem állít be induláskor eltérő
engedélyezési módot. A két szolgáltató engedélyei külön maradnak.

## Helyi diktálás CLI-panelekbe

Tartsd nyomva a **Menü** (vagy az **F8**) billentyűt a beszédhez: felengedéskor a felvétel lezárul és a felismert szöveg beíródik. Az alsó **Diktálás** gombbal egérrel is indítható és leállítható
a mikrofonfelvételt. Válaszd ki előtte a kívánt, futó CLI-panelt. A magyarul felismert
szöveg a felvétel kezdetén kiválasztott CLI-folyamatba kerül, Enter nélkül; elküldés
előtt szerkeszthető. Panelváltás nem irányítja át a diktálást. Bezárt vagy újraindított
CLI esetén a felület jelzi a beillesztési hibát és megmutatja a felismert szöveget.

A **Menü** (Context Menu) billentyű lenyomva tartásával közvetlen push-to-talk módban diktálhatsz.
Ha a billentyűzeten nincs külön Menü gomb, használd az F8-at, vagy kattints a **Menü / F8**
gombra és válassz másikat.
A választás újraindítás után is megmarad. Az alkalmazásnak
előtérben kell lennie. Fókuszvesztés lezárja a felvételt; a **Megszakítás** gomb
eldobja a diktálást. A felvétel legfeljebb 2 perc.
Felengedés után még 200 ms hangot rögzít a szóvégek megőrzéséhez.
A felismerő magyar–angol mintaszöveget kap; a betűzött `C, L, I`,
`cé el í` és `szí el áj` alakokat `CLI`-vé állítja össze.
Az egyértelmű nagybetűs betűzés más gyakori rövidítéseknél is működik
(például `A, P, I` → `API`). A hasonló hangzású magyar szavakat,
például a „Nézd” és „Nézz” alakokat nem cseréli fel utólag.

A felismerés helyben fut a whisper.cpp motorral és a többnyelvű Whisper small
modellel. Nincs API-kulcs, hangfeltöltés vagy külön telepítendő Python. A Windows
alapértelmezett mikrofonját használja; az asztali alkalmazások mikrofonengedélye
szükséges. Az ideiglenes hangfájl feldolgozás után törlődik.

A `scripts/package.ps1` a rögzített verziójú motort és a kb. 488 MB-os modellt
letölti, SHA-256-tal ellenőrzi, majd elkészíti a `VIBE.exe` fájlt és a teljes
`dist/VIBE-windows-x64.zip` csomagot. Első csomagoláskor internet szükséges;
használatkor nem. A ZIP-et egészben kell kicsomagolni: a `speech` mappa maradjon
a `VIBE.exe` mellett. Fejlesztéshez külön a `scripts/package-speech.ps1` is futtatható.
Az egui billentyűhíd a platform által jelentett Menü (ContextMenu) gombot belsőleg a fenntartott F35
kóddal továbbítja, ezért egy valódi F35 billentyű is diktálást indít.

Saját, natív Rust asztali alkalmazás több önálló AI-beszélgetéshez. Windows alatt a **VIBE.exe** külön grafikus ablakot nyit; használatához nem kell terminál. Sötét felület, külön chatpanelek, panelenként modell és piszkozat. A korábbi terminálos felület külön, opcionális `vibe-cli` programként marad meg.

## Indítás Windows alatt

A chatválasz befejezésekor rövid Windows-értesítési hang szól, háttérben lévő
panelnél is. A normál chatek közvetlenül, a Codex és Antigravity CLI-k az
engedélyezett VIBE `Stop` hookon keresztül jeleznek. Az Antigravity megvárja
a háttérfeladatok végét. A hang a Windows értesítési hangbeállításait követi.

Nyisd meg a projekt gyökerében található **VIBE.exe** fájlt. A `Vibe.cmd` ugyanezt indítja, a `Vibe-demo.cmd` hálózat nélküli demót nyit, a `Vibe-login.cmd` a grafikus alkalmazást belépési kéréssel indítja.

Forrásból először csomagold:

```powershell
powershell -ExecutionPolicy Bypass -File .\scripts\package.ps1
```

A script kiadásra fordítja a natív `vibe` binárist, majd a kész programot a projekt gyökerébe másolja `VIBE.exe` néven. Magát az alkalmazást nem indítja el. Rust stable és Windows alatt Visual Studio C++ Build Tools, valamint Windows SDK szükséges. A resource compiler (`rc.exe`) helyét a build automatikusan keresi; egyéni SDK esetén `RC_PATH` adhatja meg teljes útvonalát.

Fejlesztői indítás:

```sh
cargo run --bin vibe -- --demo
cargo run --bin vibe -- --project .
```

## Grafikus munkafelület

- A kezdőnézetben csak az OpenRouter gomb látható belépés előtt. Saját panelnév, modellazonosító és API-kulcs adható meg.
- Sikeres ChatGPT-belépés után megjelenik a Codex CLI gomb: saját helyi Codex-folyamatot indít a projekt könyvtárában, a VIBE ablakán belül.
- Profil, belépés, kijelentkezés és beállítások bal alul. A menüben a felület mérete is állítható; alapérték 120%.
- Nagyobb ikonok, legalább 36 pontos ikonmezők és lekerekített felületek. A felső Agent / Code / Thread választó külön, tágasabb sávot kapott; jelenleg a Code nézet működik.
- Bal oldali munkatérből válassz projektet; az új panel gomb megnyitja az indítóválasztót.
- Panelenként saját kapcsolat, modell, előzmény, piszkozat és megszakítható válasz.
- Kattints a modellre a választáshoz, majd írj az adott panel üzenetmezőjébe és küldd el.
- Enter elküldi az üzenetet, Shift+Enter új sort kezd.
- Elrendezésválasztóval osztott, oszlopos és rácsos nézet között válthatsz; panel nagyítható.
- A válasz leállítása az adott panel kérését szakítja meg; a többi beszélgetés folytatódik.
- **Continue with ChatGPT** megnyitja a rendszer böngészőjében a belépést. A fiók és a modellkatalógus a grafikus felületről kezelhető.

Három kezdő panel, legfeljebb nyolc független beszélgetés. Projektmunkamenet automatikusan mentődik, induláskor visszatöltődik. Demóban kizárólag jelölt helyi mintaválaszok jelennek meg, valódi bejelentkezés és hálózati kérés nincs.

## ChatGPT-előfizetéses belépés

A **Continue with ChatGPT** gombbal jelentkezz be a rendszer böngészőjében, és engedélyezd a ChatGPT-csomag használatát a VIBE számára. Sikeres belépés után az aktív panel ChatGPT-kapcsolatra vált, a választható modellek a fiók modelllistájából érkeznek. Jelszót vagy tokent nem kell az alkalmazás üzenetmezőjébe írni.

OpenRouter-kulcs nélkül az új munkaterületek ChatGPT-panelekből indulnak. Korábbi mentett panelek kapcsolata megmarad. Minden Codex-panel ugyanazt a mentett VIBE-fiókot használja, külön terminálfolyamattal. A ChatGPT-előfizetéses kapcsolat OpenAI-modellekhez használható; Anthropic/Google-modellekhez OpenRouter-kapcsolat és saját kulcs kell.

A belépés dinamikus kliensregisztrációt használó OAuth/PKCE folyamat. Az asztali Codex-panel az eredeti interaktív `codex` programot futtatja, saját natív pszeudoterminálban (Windows: ConPTY; Linux/macOS: PTY). A teljes terminálkép, kurzor, színek, slash parancsok és engedélykérő felület a Codex CLI-ből érkezik. A VIBE saját OAuth-tokenjét csak a gyermekfolyamat környezetében adja át; külön helyi Codex-adatkönyvtárat használ, más alkalmazás belépését nem olvassa. Az opcionális `vibe-cli` továbbra is közvetlen Responses API-adaptert használ. A fiók jogosultságai és használati korlátai érvényesek; sikeres bejelentkezés önmagában nem garantál hozzáférést minden modellhez. [OpenAI belépési dokumentáció](https://developers.openai.com/siwc/token-sharing-open-source/sign-in), [modellek és inference](https://developers.openai.com/siwc/token-sharing-open-source/models-and-inference).

Tokenek: Windows alatt felhasználóhoz kötött DPAPI-val titkosított helyi fájl; Unix rendszeren tulajdonosi jogosultságú fájl (`0600`), zárt könyvtárban (`0700`). A külön `auth/` könyvtár a helyi alkalmazásadatok alatt található. A beszélgetési JSON-fájlok nem tárolják a bejelentkezés tokenjeit. Tokenfrissítés automatikus, folyamatok között zárolt. Kijelentkezés megpróbálja visszavonni a megújítható munkamenetet, majd eltávolítja a helyi tokeneket; a host- és kliensregisztráció megmarad. Ha a távoli visszavonás nem igazolható, a felület jelzi.

Éles OAuth és modellhozzáférés csak saját böngészős engedélyezés után ellenőrizhető. Automatikus tesztek nem olvassák a valódi fiók hitelesítőit, és nem indítanak fizetős modellkérést.

## Saját OpenRouter-kulcs

A kezdőképernyő OpenRouter gombjánál vagy a bal alsó beállításokban add meg a panel nevét, az OpenRouter modellazonosítót és a maszkolt API-kulcsot. A kulcs kizárólag a futó alkalmazás memóriájában marad; újraindítás után újra meg kell adni. Alternatíva az örökölt `OPENROUTER_API_KEY` környezeti változó. Modellpélda: `openrouter/auto`; a választott modellhez szükséges szolgáltatói hozzáférést az OpenRouter-fiókod biztosítja.

```powershell
.\VIBE.exe --project .
```

OpenRouter módban Claude-, OpenAI- és Gemini-modellek is OpenRouteren keresztül érhetők el. Az elérhetőség és a díjazás változhat; a modell panelenként választható. A közvetlen OpenAI API-kulcsos adapter későbbi fejlesztés; a ChatGPT-előfizetéses mód saját belépést használ.

## Mentés és adatkezelés

A munkamenet verziózott JSON-fájlba mentődik atomikus fájlcserével; SQLite későbbi migráció. Az üzenetek és piszkozatok titkosítás nélkül, helyi alkalmazásadat-könyvtárban tárolódnak:

- Windows: `%LOCALAPPDATA%\Vibe\Vibe\data\sessions\`
- Linux: `$XDG_DATA_HOME/vibe/sessions/`, alapértelmezetten `~/.local/share/vibe/sessions/`.
- macOS: a felhasználó `Library/Application Support` könyvtára alatt.

Fájlnév: a kanonikus projektútvonal SHA-256 azonosítója. Ugyanazt a projektmunkamenetet egyszerre egy mentő példány nyithatja meg. A `--no-save` nem tölt és nem ment beszélgetést; a demó is elkülönül a mentett munkamenetektől. Sérült vagy ismeretlen verziójú fájlt a program nem ír felül. Projekt átnevezése vagy mozgatása új munkamenet-azonosítót ad. Megszakadt futás részleges válasza helyreállításkor jelölést kap.

OpenRouter-kérés az adott panel korábbi felhasználói és asszisztensüzeneteit továbbítja a szolgáltatónak. A Codex CLI saját szálat és eszközhasználatot kezel: a projektfájlokhoz a kiválasztott munkakönyvtárban férhet hozzá, és a feladat alapján forrásrészleteket küldhet a modellnek. Más panelek beszélgetése külön marad. A Codex eszközműveleteire a megjegyzett jogosultsági mód, ennek hiányában a CLI saját konfigurációja érvényes; az engedélykérésekre a Codex eredeti terminálfelületén válaszolsz.

OpenRouter-kérésenként 2048 kimeneti token felső korlát; díjazás saját OpenRouter-fiókban történik. ChatGPT-módban a csomag jogosultságai és kerete érvényesülnek, az integráció nem küld külön kimeneti tokenkorlátot. A felület használati adatokat mutat, saját előfizetéses kreditet még nem kezel. Megszakítás után is elszámolódhat szolgáltatói használat.

## Opcionális terminálos változat

A korábbi Ratatui felület külön bináris. A natív asztali változat az alapértelmezett termék.

```sh
cargo run --bin vibe-cli -- --demo
cargo run --bin vibe-cli -- login
cargo run --bin vibe-cli -- account
cargo run --bin vibe-cli -- logout
cargo run --bin vibe-cli -- --provider chatgpt
cargo run --bin vibe-cli -- --demo --snapshot --width 160 --height 48
```

CLI billentyűk: Tab / Shift+Tab panelváltás, Ctrl+N / Ctrl+W panel nyitás/bezárás, Ctrl+M modell, Ctrl+O ChatGPT-belépés, Ctrl+P kapcsolatváltás, Ctrl+L elrendezés, Ctrl+Z nagyítás, Enter küldés, Alt+Enter új sor, Esc megszakítás, Ctrl+Q kilépés. Fiókparancsok: `/login`, `/logout`, `/models`. Panelparancsok: `/model <azonosító>`, `/provider chatgpt`, `/provider openrouter`, `/title <név>`, `/clear`, `/new`, `/help`.

## Fejlesztés

```sh
cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo test --all-targets
cargo build --release --bin vibe
cargo build --release --bin vibe-cli
```

Windows alatt a teljes beágyazott Codex-felület indulása és gyors átméretezése külön, elakadásfigyelős integrációs teszttel is ellenőrizhető:

```sh
cargo test --lib installed_cli_full_widget_keeps_frames_responsive -- --ignored --nocapture
```

Telepített Codex CLI szükséges. A teszt elkülönített könyvtárat és szintetikus hitelesítőt használ; `/` karaktert ír a menü megnyitásához, modellkérést nem indít. A terminál I/O és méretezés háttérszálon fut; a felület kész képernyőállapotot vesz át.

A CI Linux, Windows és macOS rendszereken futtat buildet és teszteket. Windows alatt a kiadási EXE saját ikont és verzióadatot kap. Linux/macOS build nem futtat Windows resource compilert. A `scripts/package.ps1` Windows-csomagolásra szolgál.

```text
src/desktop.rs   Natív eframe/egui alkalmazás, külön grafikus ablak
src/app.rs       Közös panelállapot, inputok, modellek és elrendezés
src/provider.rs  OpenRouter streaming, SSE-feldolgozás és demó
src/chatgpt.rs   Előfizetéses Responses streaming és modelllista
src/auth.rs      Böngészős OAuth, JWT-ellenőrzés, frissítés, kijelentkezés
src/auth_store.rs Védett tokenek, host/regisztráció és folyamatközi zár
src/terminal_backend.rs Natív PTY/ConPTY, eredeti interaktív Codex CLI
src/terminal_widget.rs Terminálkép, ANSI-színek, kurzor, billentyűk és átméretezés
src/storage.rs   Verziózott helyi munkamenetek, validálás, atomikus mentés
src/main.rs      Opcionális vibe-cli, terminálos eseményciklus
src/ui.rs        Opcionális Ratatui felület
build.rs         Windows EXE-ikon és termékverzió
scripts/package.ps1 Native release build és VIBE.exe csomagolás
```

## További fejlesztés

### Google-belépés és Antigravity CLI

A Google usage-kártya az Antigravity CLI strukturált parancsválaszának
`command.data.groups` mezőjéből is kiolvassa a modellkereteket (CLI 1.4.2).
A szöveges `response` nem kvótaadat. Sikertelen vagy agentkört tartalmazó válaszból
nem jelenít meg friss keretet; a korábbi adatot hibajelzéssel megőrzi.

A felső sáv Google-ikonja alatt külön **Antigravity usage** kártya mutatja a CLI által jelentett
modellkereteket, a maradék százalékot és a visszaállás idejét. A lekérés az
Antigravity saját, csak olvasó `/usage` parancsát használja; nem másolja ki a
Google-tokent, és nem indít modellkérést. Az ikon megnyomása vagy a frissítés
gomb lekérdez; automatikus háttérfrissítés nincs. Sikertelen lekérés nem
indít automatikus belépési folyamatot vagy végtelen újrapróbálást. A korábbi
adat ilyenkor gyorsítótárazottként szerepel. Hiányzó érték nem jelent nulla keretet.

Az integráció legalább Antigravity CLI 1.1.11-et igényel. A fejlesztői tesztek
mintaválaszokat használnak; élő Google-fiókkal a kvótakijelzést külön ellenőrizni
kell. [Hivatalos kvótadokumentáció](https://antigravity.google/docs/cli/commands/usage).

A kezdőlapon az **Antigravity CLI**, a bal alsó profilmenüben a **Continue with
Google · Antigravity** gomb nyit külön eredeti Antigravity terminálpanelt. Ehhez
nem szükséges ChatGPT-bejelentkezés. Az `agy` saját böngészős Google-belépését és
a rendszer hitelesítőtárát használja; a VIBE nem kér és nem tárol Google-jelszót
vagy saját Google-tokent. Meglévő helyi Antigravity-belépést a CLI újrahasználhat.

A Google/Antigravity gomb megnyomásakor a VIBE megkeresi az `agy` CLI-t. Ha
hiányzik, a hivatalos kiadási jegyzék alapján letölti a felhasználói könyvtárba,
SHA-512-vel ellenőrzi, majd elindítja a Google-belépést. Letöltési folyamatot és
hibát a panel mutat; a panel bezárása megszakítja a letöltést. Meglévő CLI-t nem
ír felül. Windows alatt `%LOCALAPPDATA%\agy\bin\agy.exe` a telepítési hely,
Linux/macOS alatt `~/.local/bin/agy`. Nem kell külön terminálos telepítés.
[Hivatalos telepítési forrás](https://antigravity.google/docs/install?tab=cli).

Az Antigravity `/model` parancsával választható modell, `/logout` paranccsal
kezelhető a Google-kijelentkezés. A ChatGPT-kijelentkezés nem állítja le az
Antigravity-paneleket. A workspace-váltás és az app bezárása minden panelt lezár.

### Profil és workspace-ek

A bal alsó profilmenü **Continue with ChatGPT / Codex** gombja az eredeti Codex
CLI hivatalos böngészős bejelentkezését indítja. A VIBE saját, tartós
`native-codex` profiljába jelentkezel be; más alkalmazás fiókját nem olvassuk.
Ugyanezt a profilt használja a Codex terminál és az
[`account/rateLimits/read`](https://learn.chatgpt.com/docs/app-server) usage-lekérés.
A felső OpenAI-ikon lenyíló kártyája az öt órás/heti keretet, a maradék százalékot és a visszaállás idejét mutatja.
A **Disconnect Codex account** megszakítja e fiók Codex-paneljeit és kijelentkeztet;
a Google/Antigravity kapcsolat ettől független.

A korábbi ChatGPT token-sharing kapcsolat megmarad tartaléknak, de usage-hoz
natív Codex-belépés szükséges. A VIBE nem próbál többé ilyen tokent a privát
HTTP usage végpontra küldeni. Az első natív Codex-kapcsolatot a felhasználónak a
böngészőben kell jóváhagynia. A korábbi használati adat időbélyeggel,
gyorsítótárazottként jelenik meg, ha a frissítés sikertelen.

A workspace sor végén lévő hárompontos menüben kitűzés és listából eltávolítás
érhető el. A kitűzöttek felül jelennek meg. Az eltávolítás megtartja a projekt
fájljait és a mentett beszélgetéseket; az aktív futásokat megállítja. A lista
állapota újraindítás után is megmarad; a mappa újranyitása visszahozza a bejegyzést.

### Képernyőmentés beillesztése

Windows alatt készíts képernyőmentést a `Win+Shift+S` billentyűkkel, kattints a
Codex terminálpanelbe, majd nyomj `Ctrl+V`-t. A képet az eredeti Codex CLI olvassa
ki a vágólapról és csatolja a szerkesztett üzenethez. Beillesztéskor nincs
automatikus küldés; az üzenetet külön az Enter küldi el. Szöveg másolásakor a
megszokott szöveges beillesztés működik. A képvágólap támogatásához helyi,
interaktív Codex CLI szükséges.

Az `egui-winit` helyi javítása (`vendor/egui-winit/VIBE-PATCH.md`) megőrzi a
Ctrl+V billentyűeseményt, ha a vágólapon nincs szöveg. A képfeldolgozás a Codex
folyamatában történik, a VIBE nem olvassa vagy másolja külön a képet.

A Codex CLI panel az eredeti interaktív terminálfelületet mutatja, nem egy átformázott AI-chatet. Panelenként külön Codex-folyamat fut a kiválasztott projektben. Kattintás után közvetlenül a terminálba írhatsz; Enter, nyilak, Tab és a Codex slash parancsai a programhoz jutnak. Mindkét natív CLI-panelben a Ctrl+C és Ctrl+Shift+C a kijelölt szöveget másolja; kijelölés nélkül a látható terminálszöveget. A Ctrl+C nem küld megszakítást, dupla lenyomása sem lépteti ki a CLI-t. Futó művelet megszakításához használd a CLI Esc billentyűjét. Átméretezéskor a terminál sor- és oszlopszáma is változik. A panel fejlécében újraindítás, nagyítás és bezárás érhető el. Az OpenRouter továbbra is saját chatfelületet használ.

Az OpenRouter-chatek, piszkozatok és a natív CLI-panelek helyben mentődnek. Újraindításkor a terminál képe, színei és legfeljebb 1000 sor / 1 MiB görgethető szöveges előzménye folyamatindítás nélkül visszatöltődik. A fejléc ▶ gombja folytatja az eredeti beszélgetést: Codex `resume <azonosító>`, Antigravity `--conversation <azonosító>`. Induláskor nincs automatikus CLI- vagy hálózati indítás. A mentett terminálkép a folytatásig csak olvasható. A látható, még el nem küldött szöveget nem gépeli vissza automatikusan; bezárás leállítja a futó feladatot.

A natív beszélgetésazonosítót SessionStart hook rögzíti, panelenként külön. Codexben az új hookokat egyszer a `/hooks` felületen jóvá kell hagyni; a következő befejezett válasznál vagy normál kilépéskor is rögzül az azonosító. A VIBE nem kerüli meg ezt az eredeti bizalmi ellenőrzést. [Codex hook-jóváhagyás](https://learn.chatgpt.com/docs/hooks#review-and-trust-hooks). Codex-azonosító hiányában az eredeti munkamenetválasztó nyílik. Antigravity esetén az újonnan induló CLI-ben `/resume` paranccsal választható korábbi beszélgetés, ha az azonosító még nem ismert. A Google-hook az első kézi Antigravity-indításkor kerül a meglévő `.gemini/config/hooks.json` fájlba, a többi beállítás megtartásával; VIBE-en kívül inaktív. Google-előfizetéses élő folytatási teszt még szükséges.

Terminálmentések: a helyi alkalmazásadat-könyvtár `terminals` almappájában, titkosítás nélkül; tokeneket a hook nem ment. Pillanatkép változáskor két másodpercenként és bezáráskor készül. A `--no-save` és demó nem tölt vagy ment ilyen adatot. Sérült mentést nem ír felül. Korábbi, e funkció nélküli futások elveszett terminálképe nem állítható helyre. A Codex eredeti jóváhagyási felülete működik.

A Codex CLI-nek telepítve kell lennie. A program natív `codex`/`codex.exe` binárist vagy az npm Codex-csomag natív futtatható állományát keresi; hiány esetén a felület hibát jelez. Demóban nem indít Codexet és nem használ valódi tokent. [Hivatalos Codex CLI](https://learn.chatgpt.com/docs/codex/cli).

A használati kártyák a felső Agent / Code / Thread kapcsoló mellett, külön
OpenAI- és Google-ikon alatt találhatók. A bal alsó profilmenü belépést és
beállításokat tartalmaz, használati lekérést nem indít.

Google-belépés megőrzése: az eredeti Antigravity CLI a rendszer hitelesítéstárába menti a munkamenetet, és következő indításkor újra felhasználja. A VIBE külön megjegyzi a Google-kapcsolat beállítását, ezért az ikon terminálpanel nélkül is megmarad, a mentett munkamenetet pedig a Google ikonra kattintáskor, csak olvasható `/usage` lekéréssel ellenőrzi. Lejárt vagy törölt hitelesítésnél a CLI-ben újra be kell lépni; a beállítás önmagában nem jelent érvényes bejelentkezést. Kijelentkezés: Antigravity CLI `/logout`. Google-token nem kerül a VIBE munkamenetfájljaiba.

Hálózatkímélő indulás: az üres asztali ablak csak helyi mentett fiókadatokat olvas. Nem indít Codex/Antigravity háttérfolyamatot, nem kér le automatikusan modelllistát vagy usage-adatokat; nincs perces usage-polling. A limitek a fejléc szolgáltatóikonjára vagy a frissítésre kattintva töltődnek be. A CLI-panel indítása továbbra is a kiválasztott CLI saját hálózati működésével jár.

Asztali induláskor a VIBE a CLI-választót mutatja. Első telepítéskor a workspace-lista üres; nem készít alapértelmezett projektet vagy mintapanelt, és nem használja az indítófolyamat munkakönyvtárát. CLI-választás után kézzel kell megadni a projektmappát, szükség esetén bejelentkezéssel. A mentett projektek a bal oldali listából megnyithatók, beszélgetéseik megmaradnak. A korábbi System32-bejegyzést a lista kiszűri, a fájlok és mentések törlése nélkül. A `--project` explicit projektindítás; a `--demo` külön mintanézet. A `--no-save` nem menti az új munkamenetet. Ez a 0.2.3-as működés felülírja a korábbi automatikus legutóbbi-projekt indítást.

Usage-frissítés: a felső OpenAI-/Google-ikon megnyitásakor pontosan az adott szolgáltató adatait kéri le. Az ikonra kattintva történő bezárás, a nyitva tartás és az app indulása nem indít usage-lekérést. A kártya frissítés gombja külön kézi lekérés; az egyszerre futó azonos szolgáltatói lekéréseket összevonja.

Az asztali alkalmazásban az utolsó panel is bezárható. A hozzá tartozó CLI és a folyamatban lévő indítás leáll; az üres workspace kezdőnézete jelenik meg új panel automatikus létrehozása nélkül. Az üres panelállapot menthető és visszatölthető.

A natív CLI-panelek kompakt, 14-es monospace betűméretet és a monitor DPI-jéhez igazított, fizikai képpontra illesztett karakterrácsot használnak. A karakterek, a kurzor és a kijelölés ugyanazt a rácsot követik.

CLI bevitel: a Ctrl+Alt az AltGr-rel bevitt, billentyűzetkiosztás szerinti karaktereket is átengedi; nem indít helyettük másolás/beillesztés gyorsbillentyűt. Felismert, látható `› `, `❯ ` vagy `> ` CLI-bevitelnél a Ctrl+A kijelöli a beviteli szöveget. Backspace/Delete törli, gépelés vagy szövegbeillesztés felülírja, Ctrl+C másolja. A kijelölés a látható beviteli régióra vonatkozik; más CLI-nézetek eredeti billentyűkezelése megmarad. A megvalósítás nem küld Entert vagy Ctrl+C megszakítást a törléshez.


### Modern asztali megjelenítés

Egységes neutrális panel- és menüstílus, finom aktív szegélyek, változatlan panelkiosztás. Animált panel-fókusz, ikon-, menüsor- és modellkártya-hover, egységes lekerekített popupok árnyékkal. A terminál rögzített 14 pontos, fizikai képponthoz igazított méretet használ; nincs betűméret-választó. Windows alatt a helyben telepített Consolas normál és valódi félkövér betűkészlete kerül betöltésre, nem készül eltolásos dupla betűrajzolás. Más rendszeren a csomagolt monospace a tartalék. Visszagörgetéskor a Legfrissebb gomb az élő nézetre ugrik. A natív CLI-bemenet és engedélykérések megmaradnak.


### Jóváhagyott látványterv átültetése

A natív CLI-panelek egységes logós, kétsoros fejlécet kaptak. A hárompontos menü tartalmazza a folytatást/újraindítást, átnevezést, leírást, modell- és gondolkodásbeállításokat, mentést és bezárást. A terminál fölötti külön állapotsáv megszűnt. A felső és jobb oldali nézetválasztók aláhúzásos megjelenésűek; a jobb oldali dashboard valódi panelszámokat és állapotokat mutat, a látványterv mintaadatai nélkül. Usage-fülek között választáskor kézi lekérés történik, háttérfrissítés nincs. A Browser továbbra is a korábbi helyi helyőrző; ez a dizájnfrissítés nem telepít webmotort.

Offline grafikus ellenőrzés: `cargo run --example offline_design_preview -- output/design-preview.rgba`. Ez a segéd kizárólag szintetikus paneladatokat és helyi terminálképet rajzol rejtett ablakban; nem indít CLI-t, nem olvas fiókot, nem használ hálózatot. A fájl első 8 bájtja a képméret két little-endian u32 értékként; utána RGBA képadatok.


Usage és workspace-fa: a felső OpenAI-ikon mellett a legutóbb lekért elsődleges (általában 5 órás) keret hátralévő százaléka jelenik meg, nem a heti érték. Google-nál a rendelkezésre álló modellkvóták legkisebb hátralévő százaléka látszik; a részletek modellként olvashatók. Ismeretlen adat helyén gondolatjel szerepel, a 0% valódi nullának számít. A fiókkártyán külön Account & usage fejléc, logós fülek, ötórás/heti kék keretsávok, reset-idők és fix kézi frissítési lábléc van. Nincs új automatikus hálózati művelet. A workspace-lista csoportfát mutat mappa- és beszélgetésikonokkal, külön lenyitható csoportokkal; az inaktív csoportok beszélgetései is elérhetők. Átnevezés és csoportba mozgatás jobb kattintással, új csoport a fa alján.

A kártya offline képernyőképe: `cargo run --example offline_design_preview -- output/usage-preview.rgba --usage`. A 72% és 86% csak ennek a külön offline segédnek szintetikus adata, nem kerül a normál appba.


Usage-popup javítás: a szélesség minden rajzoláskor korlátozott (380 px külső szélesség, kisebb képernyőn arányosan kevesebb); a kitöltő fülek és sávok nem növelhetik visszacsatolással a popupot. Regressziós teszt két ablakméreten összesen 600 képkockán ellenőrzi a stabil méretet és a háttérfrissítés hiányát. A teljes csoportsor kattintása egyszerre nyit/csuk és választ csoportot, visszaállítva annak teljes panelnézetét. A csoportbeállítások és a beszélgetés-áthelyezési menü közös, ikonos stílust használ; a csoport eltávolítása továbbra is megtartja a beszélgetéseket. Offline ellenőrzéshez a preview segéd támogatja a `--group-menu` és `--move-menu` kapcsolókat is.


Élő OpenAI-kijelzés: a két szolgáltató ikonja külön kártyát nyit, a belső szolgáltatóválasztó megszűnt. Google alatt is a közös kék keretsávok jelennek meg; nincs 210 px magas beágyazott görgetőlista. A teljes kártya csak akkor görgethető, ha a kvóták nem férnének el a képernyőn. Mindkét kártyán külön kézi frissítési lábléc van.

Az OpenAI-keret a futó Codex-panelek új terminálkimenete után automatikusan frissül, legfeljebb 30 másodpercenként, egyetlen közös lekéréssel. Az eredeti `account/rateLimits/read` módszert használja; nem becsüli tokenekből a százalékot. Változatlan/bezárt terminál, mentett nézet, demó és fiók nélküli ablak nem indít ilyen frissítést. Hiba után is megmarad a 30 másodperces korlát. A Google automatikus lekérése változatlanul kézi; az élő frissítés az OpenAI-ra vonatkozik. Ez felülírja a korábbi, kizárólag kézi OpenAI-frissítésről szóló leírást. Az OpenAI fiókesemények dokumentációja: https://learn.chatgpt.com/docs/app-server .


### Továbbküldés másik gépre

Diktálással együtt a `dist/VIBE-windows-x64.zip` teljes csomagot küldd el. A fogadó fél bontsa ki az egész ZIP-et egy mappába, majd onnan indítsa a `VIBE.exe` fájlt. Az exe mellett maradjon a `speech` mappa a modellel és a DLL-ekkel. Az önálló exe a diktálási modellt nem tartalmazza. A végfelhasználónak nem kell Rustot telepítenie vagy csomagolóscripteket futtatnia. Csomagolás már előkészített helyi eszközökkel, beszédfelismerő-letöltés nélkül: `./scripts/package.ps1 -UseExistingSpeech`. Ez ellenőrzi a kötelező fájlokat és a modell SHA-256 összegét.

### OpenRouter VIBE Agent

Az asztali OpenRouter-panel saját, CLI-szerű kódoló ügynököt futtat. Az eredeti Codex/Antigravity PTY terminálokat nem helyettesíti. Az OpenRouter-modellnek támogatnia kell az eszközhívást; a kérések a `tools` paramétert támogató végpontokra korlátozódnak. Helyi parancsok: `/help`, `/clear`, `/model`, `/model szolgáltató/modell`.

Eszközök: `list_directory`, `read_file`, `search_files`, `write_file`. A workspace-en belüli relatív útvonalakat kezeli; az olvasott fájlrészleteket a kiválasztott OpenRouter-szolgáltatóhoz küldi. Nem tölti fel automatikusan a teljes projektet. Kizárja a tipikus titokfájlokat és generált könyvtárakat, de ez nem teljes körű titokfelismerés. UTF-8 szövegfájlok legfeljebb 256 KiB méretig, korlátozott keresés és eszközeredmény. Egy feladat legfeljebb 16 modellkör / 32 eszközhívás; modellválaszonként legfeljebb 8192 kimeneti token. Ez több fizetős API-kérést jelenthet egyetlen felhasználói promptból.

A fájlírás előnézetet és minden módosításhoz külön jóváhagyást kér. Az eredeti tartalom megváltozása esetén nem ír felül, a mentés ideiglenes fájl és atomikus csere használatával történik. Új fájlnál a szülőmappának léteznie kell. Nincs shell/parancsfuttatás, fájltörlés vagy tetszőleges külső mappahozzáférés. Az előnézet jóváhagyása nem mentett engedély; újraindításkor nem hajt végre függő módosítást. A műveletnapló a chat helyi mentésének része; tartalmazhat olvasott fájlrészleteket. A részletes tool-protokoll csak az aktuális feladaton belül él, későbbi feladatnál újraolvassa a szükséges fájlokat.

Offline felületi ellenőrzés: `cargo run --example offline_design_preview -- output/openrouter-preview.rgba --openrouter`, illetve `--edit-review`. Kizárólag szintetikus adatok, nincs modellhívás vagy projektfájl-módosítás. További UI-terv: `docs/UI-fejlesztesi-terv.md`.

### Projektfájlok, Git-nézet és csatolmányok (0.2.2)

A jobb oldali **Files** fül valódi, lusta betöltésű mappafát mutat. A mappák csak megnyitáskor töltődnek, legfeljebb 500 bejegyzéssel; a névszűrő a betöltött fájlnevekre vonatkozik. A fájl előnézete másolható és sorszámozott. Az UTF-8 szövegfájlok 256 KiB-ig nyithatók meg; titokfájlok, generált mappák és a workspace-ből kivezető útvonalak kizártak. A felület lemezműveletei háttérfeladatok, nem a rajzolási szálon futnak. Nincs induláskori fájlbejárás.

A **Changes** fül helyi Git-állapotot és fájlonként színezett diffet mutat, külön index- és munkafájl-eltéréssel. A Gitnek telepítve kell lennie; hiánya csak ezt a nézetet érinti. Az állapot a fül első megnyitásakor és kézi Frissítésre töltődik, nincs polling, fetch vagy push. Külső diff, textconv, fsmonitor és clean/process filter nincs végrehajtva. Ezért például Git LFS-nél a nyers fájl és index összehasonlítása látszhat. A Git-műveletek idő- és kimenetkorlátosak; a nézet nem stage-el, commitol, állít vissza vagy töröl fájlt. A Changes lista legfeljebb 500 tételt mutat, a Git-kimenet 512 KiB, a diff-előnézet 256 KiB méretig terjed.

A fájl előnézetében a **Csatolás az aktív OpenRouter-prompt mellé** gomb a kiválasztott OpenRouter-panelhez adja a fájlt. A bevitel feletti chipek kattintással eltávolíthatók. Legfeljebb 8 fájl / összesen 64 KiB csatolható; a tartalmat az app küldéskor újra beolvassa és validálja. Hibás/hiányzó vagy túl nagy fájlnál nem indít modellhívást, a piszkozat és a csatolás megmarad. Az útvonalak a piszkozatokkal együtt mentődnek, automatikus visszaolvasás vagy feltöltés nélkül. A sikeres küldéskori tartalom a beszélgetés részeként mentődik és a kiválasztott modellszolgáltatóhoz kerül; az előzményben külön lenyitható. A natív Codex/Antigravity CLI saját fájlkezelése megmarad.

Offline UI-ellenőrzés: az `offline_design_preview` példa `--files`, `--changes`, `--file-preview`, `--diff-preview` kapcsolói szintetikus fájladatokat rajzolnak hálózat és projektfájl-bejárás nélkül.

### Windows telepítő és GitHub-frissítések

Ajánlott terjesztés: [VIBE Releases](https://github.com/Szilaay/Vibe/releases/latest). A `VIBE-Setup-x.y.z.exe` tartalmazza az offline diktálás modelljét, a beszédfelismerőt és az app-local Microsoft futtatókönyvtárakat. Windows 10/11 x64, felhasználónkénti telepítés, rendszergazdai jogosultság nélkül. A telepítő és az eltávolító nem törli az alkalmazásadatok között tárolt beszélgetéseket vagy fiókbeállításokat. A korábbi portable verzióról egyszer kézzel kell telepíteni.

Az alkalmazás indulás után legalább 20 másodperccel, majd legfeljebb hatóránként ellenőrzi a kis kiadási leírót; az ellenőrzés idejét újraindítások között is megőrzi. Ez külön, új hálózati művelet a korábbi offline induláshoz képest. Kézi ellenőrzés a profilmenüben. Teljes telepítőletöltés és telepítés külön kattintásra történik. Hálózati vagy ellenőrzési hiba nem akadályozza az app használatát. Demóban nincs frissítési hálózati művelet.

A buildbe rögzített Ed25519 nyilvános kulcs ellenőrzi a `latest.json` aláírását, majd a letöltött telepítő méretét és SHA-256 összegét. Ez nem Windows Authenticode aláírás: Windows kiadói tanúsítvány egyelőre nincs. Új kiadáshoz emeld a `Cargo.toml` verzióját; már publikált verziót ne cserélj le.

Fejlesztői kiadásmenet:

1. Hivatalos NSIS 3.13 fordító, Visual Studio C++ redisztribúciós fájlok és az előkészített `speech` mappa szükségesek.
2. `./scripts/installer.ps1` lebuildeli az alkalmazást, létrehozza a telepítőt és aláírja a `dist/latest.json` leírót. Más NSIS útvonal: `-Compiler C:/path/makensis.exe`.
3. `./target/release/vibe-release.exe verify dist/latest.json dist/VIBE-Setup-x.y.z.exe` ellenőrzi a kész csomagot.
4. `python scripts/publish-release.py --publish` a Git Credential Managerben tárolt GitHub-fiókkal feltölti a telepítőt, a leírót és a nyilvános kulcsot, majd publikálja a kiadást. A kapcsoló nélkül piszkozat marad. Forrást, fiókadatot és privát kulcsot nem tölt fel.

A kiadási privát kulcs a fejlesztői Windows-fiókhoz/géphez kötött DPAPI-védelemmel, a helyi alkalmazásadatok `release-signing/ed25519.dpapi` fájljában van. A `release/update-public-key.txt` kizárólag a nyilvános kulcs. A meglévő kulcsot őrizd meg: a DPAPI-fájl más gépre másolása önmagában nem működő kulcsmentés. Új kulccsal a régi telepítések nem fogadják el az új frissítéseket. A fejlesztői kulcsgenerálás egyszeri parancsa: `cargo run --bin vibe-release -- init-key`.
