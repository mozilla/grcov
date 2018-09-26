#include "llvm/Support/Errc.h"
#include "llvm/Support/FileSystem.h"
#include "llvm/Support/Path.h"
#include "llvm/ProfileData/GCOV.h"
#include "GCOVOutput.hxx"

using namespace llvm;

class CustomFileInfo : public FileInfo {
public:
  CustomFileInfo(const GCOV::Options &Options) : FileInfo(Options) {}

  void printIntermediate(StringRef WorkingDir, StringRef MainFilename);

  template<typename T>
  void printIntermediate(T &Output);
};

void CustomFileInfo::printIntermediate(StringRef WorkingDir, StringRef MainFilename) {
  std::string CoveragePath = getCoveragePath(MainFilename, MainFilename);
  SmallString<128> FullCoveragePath(WorkingDir);
  sys::path::append(FullCoveragePath, CoveragePath);
  std::unique_ptr<raw_ostream> CovOs = openCoveragePath(FullCoveragePath);
  GCOVOutputStream Output(*CovOs.get());
  printIntermediate(Output);
}

/// printIntermediate -  Print source files with collected line count information in the intermediate gcov format.
template<typename T>
void CustomFileInfo::printIntermediate(T &Output) {
  SmallVector<StringRef, 4> Filenames;
  for (const auto &LI : LineInfo)
    Filenames.push_back(LI.first());
  std::sort(Filenames.begin(), Filenames.end());

  for (StringRef Filename : Filenames) {
    Output.handleFile(Filename);

    const LineData &Line = LineInfo[Filename];
    for (uint32_t LineIndex = 0; LineIndex < Line.LastLine; ++LineIndex) {
      FunctionLines::const_iterator FuncsIt = Line.Functions.find(LineIndex);
      if (FuncsIt != Line.Functions.end()) {
        for (const GCOVFunction *Func : FuncsIt->second) {
          Output.handleFunction(LineIndex + 1, Func->getEntryCount(), Func->getName());
        }
      }

      BlockLines::const_iterator BlocksIt = Line.Blocks.find(LineIndex);
      if (BlocksIt == Line.Blocks.end()) {
        // No basic blocks are on this line. Not an executable line of code.
        continue;
      } else {
        const BlockVector &Blocks = BlocksIt->second;

        // Add up the block counts to form line counts.
        DenseMap<const GCOVFunction *, bool> LineExecs;
        uint64_t LineCount = 0;
        for (const GCOVBlock *Block : Blocks) {
          LineCount += Block->getCount();
        }

        Output.handleLcount(LineIndex + 1, LineCount);

        if (Options.BranchInfo) {
          for (const GCOVBlock *Block : Blocks) {
            // Only print block and branch information at the end of the block.
            if (Block->getLastLine() != LineIndex + 1)
              continue;

            size_t NumEdges = Block->getNumDstEdges();
            if (NumEdges > 1) {
              uint64_t TotalCounts = 0;
              for (const GCOVEdge *Edge : Block->dsts()) {
                TotalCounts += Edge->Count;
              }
              bool exec = TotalCounts > 0;
              for (const GCOVEdge *Edge : Block->dsts()) {
                bool taken = Edge->Count > 0;
                Output.handleBranch(LineIndex + 1, taken, exec);
              }
            }
          }
        }
      }
    }
  }
}

void parse_llvm_gcno_mbuf(void* RustHdl, char* working_dir, char* file_stem, MemoryBuffer* GCNO_Buff, MemoryBuffer* GCDA_Buff, uint8_t branch_enabled) {
  GCOV::Options Options(
    /* AllBlocks */ false,
    /* BranchProb (BranchInfo) */ branch_enabled != 0,
    /* BranchCount */ branch_enabled != 0,
    /* FuncSummary (FuncCoverage) */ false,
    /* PreservePaths */ false,
    /* UncondBranch */ false,
    /* LongNames */ false,
    /* NoOutput */ false
  );

  GCOVFile GF;
  std::string GCNO = std::string(file_stem) + ".gcno";

  GCOVBuffer GCNO_GB(GCNO_Buff);
  if (!GF.readGCNO(GCNO_GB)) {
    errs() << "Invalid .gcno File!\n";
    return;
  }

  if (GCDA_Buff->getBufferSize() != 0) {
      GCOVBuffer GCDA_GB(GCDA_Buff);
      if (!GF.readGCDA(GCDA_GB)) {
          errs() << "Invalid .gcda File!\n";
          return;
      }
  }

  CustomFileInfo FI(Options);
  GF.collectLineCounts(FI);
  if (RustHdl) {
      GCOVOutputRust Output(RustHdl);
      FI.printIntermediate(Output);
  } else {
      FI.printIntermediate(working_dir, GCNO);
  }
}

extern "C"
void parse_llvm_gcno(void* RustHdl, char* working_dir, char* file_stem, uint8_t branch_enabled) {
  std::string GCNO = std::string(file_stem) + ".gcno";
  std::string GCDA = std::string(file_stem) + ".gcda";
  std::unique_ptr<MemoryBuffer> gcno_buf;
  std::unique_ptr<MemoryBuffer> gcda_buf;

  ErrorOr<std::unique_ptr<MemoryBuffer>> GCNO_Buff = MemoryBuffer::getFileOrSTDIN(GCNO);
  if (std::error_code EC = GCNO_Buff.getError()) {
    errs() << GCNO << ": " << EC.message() << "\n";
    return;
  }

  gcno_buf = std::move(GCNO_Buff.get());

  ErrorOr<std::unique_ptr<MemoryBuffer>> GCDA_Buff = MemoryBuffer::getFileOrSTDIN(GCDA);
  if (std::error_code EC = GCDA_Buff.getError()) {
    if (EC != errc::no_such_file_or_directory) {
      errs() << GCDA << ": " << EC.message() << "\n";
      return;
    }
    // Clear the filename to make it clear we didn't read anything.
    GCDA = "-";
    gcda_buf = MemoryBuffer::getMemBuffer(StringRef(""));
  } else {
    gcda_buf = std::move(GCDA_Buff.get());
  }

  parse_llvm_gcno_mbuf(RustHdl, working_dir, file_stem, gcno_buf.get(), gcda_buf.get(), branch_enabled);
}

extern "C"
void parse_llvm_gcno_buf(void* RustHdl, char* working_dir, char* file_stem, char* gcno_buf, size_t gcno_buf_len, char* gcda_buf, size_t gcda_buf_len, uint8_t branch_enabled) {
    std::unique_ptr<MemoryBuffer> GCNO_Buff = MemoryBuffer::getMemBuffer(StringRef(gcno_buf, gcno_buf_len));
    std::unique_ptr<MemoryBuffer> GCDA_Buff = MemoryBuffer::getMemBuffer(StringRef(gcda_buf, gcda_buf_len));

    parse_llvm_gcno_mbuf(RustHdl, working_dir, file_stem, GCNO_Buff.get(), GCDA_Buff.get(), branch_enabled);
}
