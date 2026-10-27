#!/bin/bash
# Minecraft: Java Edition, any loader (Mod Manager)
# Installs vanilla, Fabric, Quilt, Forge, NeoForge, Paper or Purpur, chosen with MINECRAFT_LOADER.
# Switching loaders removes only the previous loader's own files; worlds, mods and configs stay.
#
# Server files: /mnt/server

# The previous loader's files are moved here, and put back if this install fails.
BACKUP=.previous-loader
MOVED=()
restore() {
  for f in "${MOVED[@]}"; do
    rm -rf "/mnt/server/$f"
    mkdir -p "/mnt/server/$(dirname "$f")"
    mv "/mnt/server/${BACKUP}/$f" "/mnt/server/$f"
  done
  rm -rf "/mnt/server/${BACKUP}"
  [ ${#MOVED[@]} -gt 0 ] && echo "Put the previous loader's files back; the server is unchanged."
}
fail() { echo -e "\n!! $1"; restore; exit 1; }
fetch() { curl -fsSL --retry 3 -A "minecraft-any-loader-egg" "$@"; }

apt-get update -qq >/dev/null 2>&1
apt-get install -y -qq curl jq >/dev/null 2>&1 || fail "Could not install curl and jq."
mkdir -p /mnt/server
cd /mnt/server || fail "No /mnt/server."

LOADER="$(echo "${MINECRAFT_LOADER:-vanilla}" | tr '[:upper:]' '[:lower:]' | tr -d ' ')"
MC_VERSION="$(echo "${MC_VERSION}" | tr -d ' ')"
LOADER_VERSION="$(echo "${LOADER_VERSION}" | tr -d ' ')"
[ -z "${LOADER_VERSION}" ] && LOADER_VERSION=latest

# --- Minecraft version (Mojang's list) and the Java it needs
MANIFEST="$(fetch https://piston-meta.mojang.com/mc/game/version_manifest_v2.json)" || fail "Could not reach Mojang."
if [ -z "${MC_VERSION}" ] || [ "${MC_VERSION}" = "latest" ]; then
  MC_VERSION="$(echo "${MANIFEST}" | jq -r '.latest.release')"
fi
VERSION_URL="$(echo "${MANIFEST}" | jq -r --arg v "${MC_VERSION}" '.versions[] | select(.id == $v) | .url')"
[ -z "${VERSION_URL}" ] && fail "Minecraft ${MC_VERSION} does not exist."
VERSION_JSON="$(fetch "${VERSION_URL}")" || fail "Could not read Minecraft ${MC_VERSION} from Mojang."
JAVA_NEEDED="$(echo "${VERSION_JSON}" | jq -r '.javaVersion.majorVersion // 8')"
VANILLA_URL="$(echo "${VERSION_JSON}" | jq -r '.downloads.server.url // empty')"
echo "Installing Minecraft ${MC_VERSION} with ${LOADER} (loader version: ${LOADER_VERSION}); it needs Java ${JAVA_NEEDED}."

# --- move what another loader left behind out of the way (never worlds, mods or configs)
rm -rf "${BACKUP}"
for f in libraries/net/minecraftforge/forge libraries/net/neoforged unix_args.txt user_jvm_args.txt run.sh run.bat \
  server.jar minecraft-server.jar fabric-server-launch.jar fabric-server-launcher.properties quilt-server-launch.jar \
  quilt-server-launcher.properties forge-*.jar neoforge-*.jar; do
  if [ -e "$f" ] || [ -L "$f" ]; then
    mkdir -p "${BACKUP}/$(dirname "$f")"
    mv "$f" "${BACKUP}/$f"
    MOVED+=("$f")
  fi
done
rm -f ./*installer*.jar ./*installer*.jar.log installer.log

vanilla_jar() {
  [ -z "${VANILLA_URL}" ] && fail "Mojang has no server download for Minecraft ${MC_VERSION}."
  fetch -o "$1" "${VANILLA_URL}" || fail "Could not download the Minecraft ${MC_VERSION} server."
}

# Java to run a loader installer with: this image has Java 21; old Forge installers want Java 8
installer_java() {
  if [ "${JAVA_NEEDED}" -le 8 ]; then
    apt-get install -y -qq openjdk-8-jre-headless >/dev/null 2>&1 || fail "Could not install Java 8 for the old Forge installer."
    echo /usr/lib/jvm/java-8-openjdk-amd64/bin/java
  else
    echo java
  fi
}

case "${LOADER}" in
vanilla)
  vanilla_jar server.jar
  ;;

fabric)
  if [ "${LOADER_VERSION}" = "latest" ]; then
    LOADER_VERSION="$(fetch https://meta.fabricmc.net/v2/versions/loader | jq -r '[.[] | select(.stable)][0].version')"
  fi
  INSTALLER="$(fetch https://meta.fabricmc.net/v2/versions/installer | jq -r '[.[] | select(.stable)][0].version')"
  echo "Fabric loader ${LOADER_VERSION}, installer ${INSTALLER}"
  fetch -o server.jar "https://meta.fabricmc.net/v2/versions/loader/${MC_VERSION}/${LOADER_VERSION}/${INSTALLER}/server/jar" \
    || fail "Fabric loader ${LOADER_VERSION} does not support Minecraft ${MC_VERSION}."
  vanilla_jar minecraft-server.jar
  echo "serverJar=minecraft-server.jar" > fabric-server-launcher.properties
  ;;

quilt)
  fetch -o quilt-installer.jar https://quiltmc.org/api/v1/download-latest-installer/java-universal || fail "Could not download the Quilt installer."
  if [ "${LOADER_VERSION}" = "latest" ]; then
    java -jar quilt-installer.jar install server "${MC_VERSION}" --download-server --install-dir=. || fail "The Quilt installer failed."
  else
    java -jar quilt-installer.jar install server "${MC_VERSION}" "${LOADER_VERSION}" --download-server --install-dir=. || fail "The Quilt installer failed."
  fi
  [ -f quilt-server-launch.jar ] || fail "The Quilt installer did not create quilt-server-launch.jar."
  mv server.jar minecraft-server.jar
  mv quilt-server-launch.jar server.jar
  echo "serverJar=minecraft-server.jar" > quilt-server-launcher.properties
  ;;

forge)
  if [ "${LOADER_VERSION}" = "latest" ]; then
    PROMOS="$(fetch https://files.minecraftforge.net/maven/net/minecraftforge/forge/promotions_slim.json)" || fail "Could not reach Forge."
    LOADER_VERSION="$(echo "${PROMOS}" | jq -r --arg r "${MC_VERSION}-recommended" --arg l "${MC_VERSION}-latest" '.promos[$r] // .promos[$l] // empty')"
    [ -z "${LOADER_VERSION}" ] && fail "Forge has no version for Minecraft ${MC_VERSION}."
  fi
  # "47.2.0" or "1.20.1-47.2.0"
  FULL="${LOADER_VERSION}"
  [[ "${FULL}" == "${MC_VERSION}-"* ]] || FULL="${MC_VERSION}-${LOADER_VERSION}"
  # 1.7.10 and 1.8.9 builds carry the Minecraft version twice
  if [[ "${MC_VERSION}" == "1.7.10" || "${MC_VERSION}" == "1.8.9" ]]; then FULL="${FULL}-${MC_VERSION}"; fi
  echo "Forge ${FULL}"
  fetch -o forge-installer.jar "https://maven.minecraftforge.net/net/minecraftforge/forge/${FULL}/forge-${FULL}-installer.jar" \
    || fail "Forge ${FULL} does not exist."
  "$(installer_java)" -jar forge-installer.jar --installServer >installer-output.log 2>&1 \
    || { tail -20 installer-output.log; fail "The Forge installer failed. With a memory limit of 0, give the server a real memory limit and try again."; }
  rm -f installer-output.log
  ARGS="$(ls libraries/net/minecraftforge/forge/*/unix_args.txt 2>/dev/null | head -1)"
  if [ -n "${ARGS}" ]; then
    ln -sf "${ARGS}" unix_args.txt
  else
    # before 1.17 Forge is a plain jar
    JAR="$(ls forge-*.jar 2>/dev/null | grep -v installer | head -1)"
    [ -z "${JAR}" ] && fail "The Forge installer did not create a server jar."
    mv "${JAR}" server.jar
  fi
  ;;

neoforge)
  if [[ "${MC_VERSION}" == "1.20.1" ]]; then ARTIFACT=forge; else ARTIFACT=neoforge; fi
  REPO="https://maven.neoforged.net/releases/net/neoforged/${ARTIFACT}"
  if [ "${LOADER_VERSION}" = "latest" ]; then
    # 1.21.1 -> 21.1.x, 1.21 -> 21.0.x, 26.2 -> 26.2.x, 1.20.1 -> 1.20.1-47.x
    if [ "${ARTIFACT}" = "forge" ]; then
      PREFIX="1.20.1-"
    else
      V="${MC_VERSION#1.}"
      [[ "${V}" == *.* ]] || V="${V}.0"
      PREFIX="${V}."
    fi
    LOADER_VERSION="$(fetch "${REPO}/maven-metadata.xml" | grep -oP '<version>\K[^<]+' | grep -F "${PREFIX}" | grep -v -- '-beta' | tail -1)"
    [ -z "${LOADER_VERSION}" ] && LOADER_VERSION="$(fetch "${REPO}/maven-metadata.xml" | grep -oP '<version>\K[^<]+' | grep -F "${PREFIX}" | tail -1)"
    [ -z "${LOADER_VERSION}" ] && fail "NeoForge has no version for Minecraft ${MC_VERSION}."
  fi
  # a pack names 1.20.1 NeoForge "47.1.106"; it is published as "1.20.1-47.1.106"
  if [ "${ARTIFACT}" = "forge" ] && [[ "${LOADER_VERSION}" != 1.20.1-* ]]; then LOADER_VERSION="1.20.1-${LOADER_VERSION}"; fi
  echo "NeoForge ${LOADER_VERSION}"
  fetch -o neoforge-installer.jar "${REPO}/${LOADER_VERSION}/${ARTIFACT}-${LOADER_VERSION}-installer.jar" \
    || fail "NeoForge ${LOADER_VERSION} does not exist."
  java -jar neoforge-installer.jar --installServer >installer-output.log 2>&1 \
    || { tail -20 installer-output.log; fail "The NeoForge installer failed."; }
  rm -f installer-output.log
  ARGS="$(ls libraries/net/neoforged/${ARTIFACT}/*/unix_args.txt 2>/dev/null | head -1)"
  [ -z "${ARGS}" ] && fail "The NeoForge installer did not create unix_args.txt."
  ln -sf "${ARGS}" unix_args.txt
  ;;

paper)
  BUILDS="$(fetch "https://fill.papermc.io/v3/projects/paper/versions/${MC_VERSION}/builds")" || fail "Paper has no builds for Minecraft ${MC_VERSION}."
  if [ "${LOADER_VERSION}" = "latest" ]; then
    BUILD="$(echo "${BUILDS}" | jq -c '[.[] | select(.channel == "STABLE")][0] // .[0]')"
  else
    BUILD="$(echo "${BUILDS}" | jq -c --arg b "${LOADER_VERSION}" '.[] | select((.id | tostring) == $b)')"
  fi
  URL="$(echo "${BUILD}" | jq -r '.downloads["server:default"].url // empty')"
  [ -z "${URL}" ] && fail "Paper build ${LOADER_VERSION} for Minecraft ${MC_VERSION} does not exist."
  echo "Paper build $(echo "${BUILD}" | jq -r .id)"
  fetch -o server.jar "${URL}" || fail "Could not download Paper."
  ;;

purpur)
  fetch -o server.jar "https://api.purpurmc.org/v2/purpur/${MC_VERSION}/${LOADER_VERSION}/download" \
    || fail "Purpur ${LOADER_VERSION} for Minecraft ${MC_VERSION} does not exist."
  ;;

*)
  fail "Unknown loader \"${LOADER}\". Use vanilla, fabric, quilt, forge, neoforge, paper or purpur."
  ;;
esac

rm -f ./*installer*.jar ./*installer*.jar.log installer.log
rm -rf "${BACKUP}"
echo -e "\nInstalled Minecraft ${MC_VERSION} with ${LOADER}. Choose a Java ${JAVA_NEEDED}+ image under Startup if the server does not start."
