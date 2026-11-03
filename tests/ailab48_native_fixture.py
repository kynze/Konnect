"""AILAB-48 native fixture builder (run with KiCad 10.0.6's bundled Python, -I).

Builds, entirely with native pcbnew APIs in a scratch directory:
  <root>/Lib.pretty/Probe.kicad_mod   library footprint whose pad has pad-local settings
  <root>/Test.kicad_pcb               board with U1 placed from that footprint, pad settings stale

usage: python -I ailab48_native_fixture.py <root>
Prints one JSON line with the library values the refresh must produce.
"""
import json
import os
import sys

import pcbnew

MM = 1_000_000


def make_pad(fp, settings):
    pad = pcbnew.PAD(fp)
    pad.SetNumber("1")
    pad.SetShape(pcbnew.PAD_SHAPE_RECTANGLE)
    pad.SetAttribute(pcbnew.PAD_ATTRIB_SMD)
    pad.SetSize(pcbnew.VECTOR2I(int(1.0 * MM), int(1.2 * MM)))
    pad.SetPosition(pcbnew.VECTOR2I(0, 0))
    pad.SetLayerSet(pad.SMDMask())
    if settings:
        pad.SetLocalSolderMaskMargin(75000)
        pad.SetLocalSolderPasteMargin(-50000)
        pad.SetLocalSolderPasteMarginRatio(-0.1)
        pad.SetLocalClearance(120000)
        pad.SetLocalZoneConnection(pcbnew.ZONE_CONNECTION_FULL)
        # SetLocalThermalSpokeWidthOverride takes std::optional<int>, which this
        # SWIG binding rejects, so the spoke-width override is not part of the fixture.
        pad.SetThermalGap(250000)
        pad.SetThermalSpokeAngleDegrees(45)
        pad.SetPadToDieLength(100000)
    return pad


def read(pad):
    zone = pad.GetLocalZoneConnection()
    return dict(
        mask=pad.GetLocalSolderMaskMargin(),
        paste=pad.GetLocalSolderPasteMargin(),
        ratio=pad.GetLocalSolderPasteMarginRatio(),
        clearance=pad.GetLocalClearance(),
        zone=None if zone is None else int(zone),
        gap=pad.GetLocalThermalGapOverride(),
        angle=pad.GetThermalSpokeAngleDegrees(),
        die=pad.GetPadToDieLength(),
        net=pad.GetNetname(),
    )


def main(root):
    lib = os.path.join(root, "Lib.pretty")
    os.makedirs(lib, exist_ok=True)

    fp = pcbnew.FOOTPRINT(None)
    fp.SetFPID(pcbnew.LIB_ID("", "Probe"))
    fp.SetReference("REF**")
    fp.Add(make_pad(fp, True))
    pcbnew.PCB_IO_KICAD_SEXPR().FootprintSave(lib, fp)

    board = pcbnew.BOARD()
    placed = pcbnew.FOOTPRINT(board)
    placed.SetFPID(pcbnew.LIB_ID("Lib", "Probe"))
    placed.SetReference("U1")
    placed.SetPosition(pcbnew.VECTOR2I(20 * MM, 15 * MM))
    placed.SetOrientationDegrees(90)
    pad = make_pad(placed, False)  # stale: library settings missing
    net = pcbnew.NETINFO_ITEM(board, "SIG1")
    board.Add(net)
    pad.SetNet(net)
    placed.Add(pad)
    board.Add(placed)
    path = os.path.join(root, "Test.kicad_pcb")
    assert pcbnew.SaveBoard(path, board), "SaveBoard failed"

    loaded = pcbnew.FootprintLoad(lib, "Probe")
    print(json.dumps(dict(version=pcbnew.Version(), library=read(list(loaded.Pads())[0]),
                          placed_before=read(list(pcbnew.LoadBoard(path).GetFootprints()[0].Pads())[0]))))


def read_board(path):
    """Native readback of the first footprint on a saved board."""
    fp = pcbnew.LoadBoard(path).GetFootprints()[0]
    pad = list(fp.Pads())[0]
    pos = fp.GetPosition()
    print(json.dumps(dict(reference=fp.GetReference(), fpid=str(fp.GetFPID().GetUniStringLibId()),
                          x=pos.x, y=pos.y, orientation=fp.GetOrientationDegrees(),
                          side="back" if fp.IsFlipped() else "front", pad=read(pad))))


if __name__ == "__main__":
    if sys.argv[1] == "read":
        read_board(sys.argv[2])
    else:
        main(sys.argv[1])
