#include "QEMMeshReduction.h"

#include "CoreMinimal.h"
#include "Features/IModularFeatures.h"
#include "HAL/PlatformProcess.h"
#include "Interfaces/IPluginManager.h"
#include "LerpVert.h"
#include "MeshBuild.h"
#include "MeshDescription.h"
#include "Modules/ModuleManager.h"
#include "OverlappingCorners.h"
#include "RenderMath.h"
#include "RenderUtils.h"
#include "StaticMeshAttributes.h"
#include "StaticMeshOperations.h"
#include "Templates/UniquePtr.h"

#include "qem_simplifier.h"

DEFINE_LOG_CATEGORY_STATIC(LogQEMMeshReduction, Log, All);
IMPLEMENT_MODULE(FQEMMeshReductionModule, QEMMeshReduction);

namespace
{
    constexpr uint32 QemExpectedAbiVersion = 6;

    void* GQemRuntimeDllHandle = nullptr;
    bool bQemApiReady = false;
    bool bLoggedApiUnavailable = false;
    void* GQemAbiContext = nullptr;
    bool bLoggedFirstReduceCall = false;

    bool LoadQemRuntimeDll(FString& OutLoadedPath)
    {
        OutLoadedPath.Empty();

        const TSharedPtr<IPlugin> Plugin = IPluginManager::Get().FindPlugin(TEXT("QEMMeshReduction"));
        if (!Plugin.IsValid())
        {
            UE_LOG(LogQEMMeshReduction, Error, TEXT("Cannot find plugin descriptor while loading qem runtime DLL."));
            return false;
        }

        const FString BaseDir = Plugin->GetBaseDir();
        const FString ThirdPartyBinDir = FPaths::Combine(BaseDir, TEXT("Source"), TEXT("ThirdParty"), TEXT("QEMSimplifier"), TEXT("Binary"), TEXT("Win64"));

        const TArray<FString> Candidates = {
            FPaths::Combine(ThirdPartyBinDir, TEXT("qem_simplifier.dll")),
        };

        for (const FString& Candidate : Candidates)
        {
            if (!FPaths::FileExists(Candidate))
            {
                continue;
            }

            void* Handle = FPlatformProcess::GetDllHandle(*Candidate);
            if (Handle != nullptr)
            {
                GQemRuntimeDllHandle = Handle;
                OutLoadedPath = Candidate;
                return true;
            }
        }

        UE_LOG(LogQEMMeshReduction, Error, TEXT("Failed to load runtime DLL qem_simplifier.dll from plugin paths."));
        for (const FString& Candidate : Candidates)
        {
            UE_LOG(LogQEMMeshReduction, Error, TEXT("Tried: %s"), *Candidate);
        }
        return false;
    }

    void CorrectAttributes(float* Attributes)
    {
        FVector3f& TangentX = *reinterpret_cast<FVector3f*>(Attributes);
        FVector3f& TangentY = *reinterpret_cast<FVector3f*>(Attributes + 3);
        FVector3f& TangentZ = *reinterpret_cast<FVector3f*>(Attributes + 6);
        FLinearColor& Color = *reinterpret_cast<FLinearColor*>(Attributes + 9);

        TangentZ.Normalize();
        TangentX -= FVector3f::DotProduct(TangentX, TangentZ) * TangentZ;
        TangentX.Normalize();
        TangentY -= FVector3f::DotProduct(TangentY, TangentZ) * TangentZ;
        TangentY -= FVector3f::DotProduct(TangentY, TangentX) * TangentX;
        TangentY.Normalize();
        Color = Color.GetClamped();
    }

    bool VertsEqual(const FLerpVert& A, const FLerpVert& B)
    {
        if (!PointsEqual(A.Position, B.Position) ||
            !NormalsEqual(A.TangentX, B.TangentX) ||
            !NormalsEqual(A.TangentY, B.TangentY) ||
            !NormalsEqual(A.TangentZ, B.TangentZ) ||
            !A.Color.Equals(B.Color))
        {
            return false;
        }

        for (int32 UVIndex = 0; UVIndex < MAX_STATIC_TEXCOORDS; ++UVIndex)
        {
            if (!UVsEqual(A.UVs[UVIndex], B.UVs[UVIndex]))
            {
                return false;
            }
        }

        return true;
    }

    class FQEMMeshReduction final : public IMeshReduction
    {
    public:
        virtual ~FQEMMeshReduction() override = default;

        virtual const FString& GetVersionString() const override
        {
            static FString Version = TEXT("QEMMeshReduction_SimplifyMesh_ABIv6");
            return Version;
        }

        virtual void ReduceMeshDescription(
            FMeshDescription& OutReducedMesh,
            float& OutMaxDeviation,
            const FMeshDescription& InMesh,
            const FOverlappingCorners& InOverlappingCorners,
            const FMeshReductionSettings& ReductionSettings) override
        {
            check(&InMesh != &OutReducedMesh);

            if (!bQemApiReady)
            {
                if (!bLoggedApiUnavailable)
                {
                    UE_LOG(LogQEMMeshReduction, Error, TEXT("qem runtime API unavailable. Returning input mesh unchanged."));
                    bLoggedApiUnavailable = true;
                }
                OutReducedMesh = InMesh;
                OutMaxDeviation = 0.0f;
                return;
            }

            if (!bLoggedFirstReduceCall)
            {
                const uint32 AbiVersion = qem_get_abi_version();
                UE_LOG(LogQEMMeshReduction, Log, TEXT("First ReduceMeshDescription call. ABI mode=v6, ABI version=%u"), AbiVersion);
                bLoggedFirstReduceCall = true;
            }

            const uint32 NumTexCoords = MAX_STATIC_TEXCOORDS;
            int32 InMeshNumTexCoords = 1;

            TArray<FLerpVert> Verts;
            TArray<uint32> Indexes;
            TArray<int32> MaterialIndexes;
            TMap<int32, int32> VertsMap;

            const FStaticMeshConstAttributes InAttributes(InMesh);
            TVertexAttributesConstRef<FVector3f> InVertexPositions = InAttributes.GetVertexPositions();
            TVertexInstanceAttributesConstRef<FVector3f> InVertexNormals = InAttributes.GetVertexInstanceNormals();
            TVertexInstanceAttributesConstRef<FVector3f> InVertexTangents = InAttributes.GetVertexInstanceTangents();
            TVertexInstanceAttributesConstRef<float> InVertexBinormalSigns = InAttributes.GetVertexInstanceBinormalSigns();
            TVertexInstanceAttributesConstRef<FVector4f> InVertexColors = InAttributes.GetVertexInstanceColors();
            TVertexInstanceAttributesConstRef<FVector2f> InVertexUVs = InAttributes.GetVertexInstanceUVs();

            int32 WedgeIndex = 0;
            for (const FTriangleID TriangleID : InMesh.Triangles().GetElementIDs())
            {
                const FPolygonGroupID PolygonGroupID = InMesh.GetTrianglePolygonGroup(TriangleID);
                TArrayView<const FVertexID> VertexIDs = InMesh.GetTriangleVertices(TriangleID);

                FVector3f CornerPositions[3];
                for (int32 TriVert = 0; TriVert < 3; ++TriVert)
                {
                    CornerPositions[TriVert] = InVertexPositions[VertexIDs[TriVert]];
                }

                if (PointsEqual(CornerPositions[0], CornerPositions[1]) ||
                    PointsEqual(CornerPositions[0], CornerPositions[2]) ||
                    PointsEqual(CornerPositions[1], CornerPositions[2]))
                {
                    WedgeIndex += 3;
                    continue;
                }

                int32 TriangleVertexIndices[3];
                for (int32 TriVert = 0; TriVert < 3; ++TriVert, ++WedgeIndex)
                {
                    const FVertexInstanceID VertexInstanceID = InMesh.GetTriangleVertexInstance(TriangleID, TriVert);

                    FLerpVert NewVert;
                    NewVert.Position = CornerPositions[TriVert];
                    NewVert.TangentX = InVertexTangents[VertexInstanceID];
                    NewVert.TangentZ = InVertexNormals[VertexInstanceID];
                    NewVert.TangentY = FVector3f::ZeroVector;

                    if (!NewVert.TangentZ.IsNearlyZero(SMALL_NUMBER) && !NewVert.TangentX.IsNearlyZero(SMALL_NUMBER))
                    {
                        NewVert.TangentY = FVector3f::CrossProduct(NewVert.TangentZ, NewVert.TangentX).GetSafeNormal() * InVertexBinormalSigns[VertexInstanceID];
                    }

                    NewVert.TangentX = NewVert.TangentX.ContainsNaN() ? FVector3f::ZeroVector : NewVert.TangentX;
                    NewVert.TangentY = NewVert.TangentY.ContainsNaN() ? FVector3f::ZeroVector : NewVert.TangentY;
                    NewVert.TangentZ = NewVert.TangentZ.ContainsNaN() ? FVector3f::ZeroVector : NewVert.TangentZ;
                    NewVert.Color = FLinearColor(InVertexColors[VertexInstanceID]);

                    for (int32 UVIndex = 0; UVIndex < (int32)NumTexCoords; ++UVIndex)
                    {
                        if (UVIndex < InVertexUVs.GetNumChannels())
                        {
                            NewVert.UVs[UVIndex] = InVertexUVs.Get(VertexInstanceID, UVIndex);
                            InMeshNumTexCoords = FMath::Max(UVIndex + 1, InMeshNumTexCoords);
                        }
                        else
                        {
                            NewVert.UVs[UVIndex] = FVector2f::ZeroVector;
                        }
                    }

                    CorrectAttributes((float*)&NewVert.TangentX);

                    const TArray<int32>& Duplicates = InOverlappingCorners.FindIfOverlapping(WedgeIndex);
                    int32 ExistingIndex = INDEX_NONE;
                    for (int32 K = 0; K < Duplicates.Num(); ++K)
                    {
                        if (Duplicates[K] >= WedgeIndex)
                        {
                            break;
                        }

                        if (const int32* Mapped = VertsMap.Find(Duplicates[K]))
                        {
                            if (VertsEqual(NewVert, Verts[*Mapped]))
                            {
                                ExistingIndex = *Mapped;
                                break;
                            }
                        }
                    }

                    if (ExistingIndex == INDEX_NONE)
                    {
                        ExistingIndex = Verts.Add(NewVert);
                        VertsMap.Add(WedgeIndex, ExistingIndex);
                    }

                    TriangleVertexIndices[TriVert] = ExistingIndex;
                }

                if (TriangleVertexIndices[0] == TriangleVertexIndices[1] ||
                    TriangleVertexIndices[1] == TriangleVertexIndices[2] ||
                    TriangleVertexIndices[0] == TriangleVertexIndices[2])
                {
                    continue;
                }

                Indexes.Add(TriangleVertexIndices[0]);
                Indexes.Add(TriangleVertexIndices[1]);
                Indexes.Add(TriangleVertexIndices[2]);
                MaterialIndexes.Add(PolygonGroupID.GetValue());
            }

            uint32 NumVerts = Verts.Num();
            uint32 NumIndexes = Indexes.Num();
            uint32 NumTris = NumIndexes / 3;

            uint32 TargetNumTris = NumTris;
            uint32 TargetNumVerts = NumVerts;

            if (ReductionSettings.TerminationCriterion == EStaticMeshReductionTerimationCriterion::Triangles ||
                ReductionSettings.TerminationCriterion == EStaticMeshReductionTerimationCriterion::Any)
            {
                TargetNumTris = FMath::CeilToInt(NumTris * ReductionSettings.PercentTriangles);
                TargetNumTris = FMath::Min(ReductionSettings.MaxNumOfTriangles, TargetNumTris);
            }

            if (ReductionSettings.TerminationCriterion == EStaticMeshReductionTerimationCriterion::Vertices ||
                ReductionSettings.TerminationCriterion == EStaticMeshReductionTerimationCriterion::Any)
            {
                TargetNumVerts = FMath::CeilToInt(NumVerts * ReductionSettings.PercentVertices);
                TargetNumVerts = FMath::Min(ReductionSettings.MaxNumOfVerts, TargetNumVerts);
            }

            TargetNumTris = FMath::Max(TargetNumTris, 64u);
            TargetNumVerts = FMath::Max(TargetNumVerts, 4u);

            if (TargetNumVerts < NumVerts || TargetNumTris < NumTris)
            {
                UE_LOG(
                    LogQEMMeshReduction,
                    Log,
                    TEXT("Simplify begin: in_verts=%u, in_tris=%u, in_indexes=%u, target_verts=%u, target_tris=%u, criterion=%d"),
                    NumVerts,
                    NumTris,
                    NumIndexes,
                    TargetNumVerts,
                    TargetNumTris,
                    static_cast<int32>(ReductionSettings.TerminationCriterion));

                const uint32 NumAttributes = (sizeof(FLerpVert) - sizeof(FVector3f)) / sizeof(float);
                float AttributeWeights[NumAttributes] = {
                    0.1f, 0.1f, 0.1f,
                    0.1f, 0.1f, 0.1f,
                    16.0f, 16.0f, 16.0f,
                };

                float* ColorWeights = AttributeWeights + 9;
                float* UVWeights = ColorWeights + 4;

                ColorWeights[0] = 0.1f;
                ColorWeights[1] = 0.1f;
                ColorWeights[2] = 0.1f;
                ColorWeights[3] = 0.1f;

                const float UVWeight = 0.5f;
                for (int32 UVIndex = 0; UVIndex < InVertexUVs.GetNumChannels(); ++UVIndex)
                {
                    float MinUV = +FLT_MAX;
                    float MaxUV = -FLT_MAX;
                    for (int32 VertexIndex = 0; VertexIndex < Verts.Num(); ++VertexIndex)
                    {
                        MinUV = FMath::Min(MinUV, Verts[VertexIndex].UVs[UVIndex].X);
                        MinUV = FMath::Min(MinUV, Verts[VertexIndex].UVs[UVIndex].Y);
                        MaxUV = FMath::Max(MaxUV, Verts[VertexIndex].UVs[UVIndex].X);
                        MaxUV = FMath::Max(MaxUV, Verts[VertexIndex].UVs[UVIndex].Y);
                    }

                    const float Range = FMath::Max(1.0f, MaxUV - MinUV);
                    UVWeights[2 * UVIndex + 0] = UVWeight / Range;
                    UVWeights[2 * UVIndex + 1] = UVWeight / Range;
                }

                float MaxErrorSqr = 0.0f;
                uint32 ResultNumVerts = 0;
                uint32 ResultNumIndexes = 0;
                uint32 ResultNumTris = 0;
                if (GQemAbiContext == nullptr)
                {
                    GQemAbiContext = qem_context_create();
                }

                if (GQemAbiContext == nullptr)
                {
                    UE_LOG(LogQEMMeshReduction, Warning, TEXT("qem_context_create failed, fallback to input mesh"));
                    OutReducedMesh = InMesh;
                    OutMaxDeviation = 0.0f;
                    return;
                }

                QemMeshView MeshView{};
                MeshView.vertices = reinterpret_cast<float*>(Verts.GetData());
                MeshView.num_vertices = static_cast<uint32_t>(Verts.Num());
                MeshView.indices = Indexes.GetData();
                MeshView.num_indices = static_cast<uint32_t>(Indexes.Num());
                MeshView.material_ids = MaterialIndexes.GetData();
                MeshView.num_attributes = NumAttributes;
                MeshView.attribute_weights = AttributeWeights;

                QemSimplifyOptions Options{};
                Options.target_vertices = TargetNumVerts;
                Options.target_triangles = TargetNumTris;
                Options.target_error = 0.0f;
                Options.min_vertices = 4;
                Options.min_triangles = 2;
                Options.limit_error = MAX_flt;
                Options.edge_weight = 512.0f;
                Options.max_edge_length_factor = 0.0f;
                Options.preserve_surface_area = 0;

                QemSimplifyResult SimplifyResult{};
                const int32 Status = qem_simplify(
                    GQemAbiContext,
                    &MeshView,
                    &Options,
                    &SimplifyResult);

                if (Status != QEM_STATUS_SUCCESS || SimplifyResult.status != QEM_STATUS_SUCCESS)
                {
                    UE_LOG(LogQEMMeshReduction, Warning, TEXT("qem_simplify failed. status=%d, result_status=%d, fallback to input mesh"), Status, SimplifyResult.status);
                    OutReducedMesh = InMesh;
                    OutMaxDeviation = 0.0f;
                    return;
                }

                MaxErrorSqr = SimplifyResult.max_error;
                ResultNumVerts = SimplifyResult.num_vertices;
                ResultNumIndexes = SimplifyResult.num_indices;
                ResultNumTris = SimplifyResult.num_triangles;

                if (ResultNumVerts == 0 || ResultNumTris == 0 || ResultNumIndexes != ResultNumTris * 3)
                {
                    UE_LOG(LogQEMMeshReduction, Warning, TEXT("simplify_mesh returned empty/invalid result, fallback to input mesh"));
                    OutReducedMesh = InMesh;
                    OutMaxDeviation = 0.0f;
                    return;
                }

                Verts.SetNum(ResultNumVerts, EAllowShrinking::No);
                Indexes.SetNum(ResultNumIndexes, EAllowShrinking::No);
                MaterialIndexes.SetNum(ResultNumTris, EAllowShrinking::No);

                NumVerts = ResultNumVerts;
                NumIndexes = ResultNumIndexes;
                NumTris = ResultNumTris;
                OutMaxDeviation = FMath::Sqrt(MaxErrorSqr) / 8.0f;

                UE_LOG(
                    LogQEMMeshReduction,
                    Log,
                    TEXT("Simplify end: out_verts=%u, out_tris=%u, out_indexes=%u, max_error_sqr=%.9g, max_deviation=%f"),
                    NumVerts,
                    NumTris,
                    NumIndexes,
                    MaxErrorSqr,
                    OutMaxDeviation);
            }
            else
            {
                OutMaxDeviation = 0.0f;
                UE_LOG(
                    LogQEMMeshReduction,
                    Verbose,
                    TEXT("Simplify skipped: in_verts=%u, in_tris=%u already meet targets (target_verts=%u, target_tris=%u)."),
                    NumVerts,
                    NumTris,
                    TargetNumVerts,
                    TargetNumTris);
            }

            OutReducedMesh.Empty();
            FStaticMeshAttributes OutAttributes(OutReducedMesh);
            OutAttributes.Register();

            const TPolygonGroupAttributesConstRef<FName> InPolygonGroupMaterialNames = InAttributes.GetPolygonGroupMaterialSlotNames();
            TPolygonGroupAttributesRef<FName> OutPolygonGroupMaterialNames = OutAttributes.GetPolygonGroupMaterialSlotNames();

            for (const FPolygonGroupID PolygonGroupID : InMesh.PolygonGroups().GetElementIDs())
            {
                OutReducedMesh.CreatePolygonGroupWithID(PolygonGroupID);
                OutPolygonGroupMaterialNames[PolygonGroupID] = InPolygonGroupMaterialNames[PolygonGroupID];
            }

            TVertexAttributesRef<FVector3f> OutVertexPositions = OutAttributes.GetVertexPositions();
            TVertexInstanceAttributesRef<FVector3f> OutVertexNormals = OutAttributes.GetVertexInstanceNormals();
            TVertexInstanceAttributesRef<FVector3f> OutVertexTangents = OutAttributes.GetVertexInstanceTangents();
            TVertexInstanceAttributesRef<float> OutVertexBinormalSigns = OutAttributes.GetVertexInstanceBinormalSigns();
            TVertexInstanceAttributesRef<FVector4f> OutVertexColors = OutAttributes.GetVertexInstanceColors();
            TVertexInstanceAttributesRef<FVector2f> OutVertexUVs = OutAttributes.GetVertexInstanceUVs();
            OutVertexUVs.SetNumChannels(InMeshNumTexCoords);

            for (uint32 VertexIndex = 0; VertexIndex < NumVerts; ++VertexIndex)
            {
                const FVertexID AddedVertexID = OutReducedMesh.CreateVertex();
                OutVertexPositions[AddedVertexID] = Verts[VertexIndex].Position;
            }

            TMap<int32, FPolygonGroupID> PolygonGroupMapping;

            for (uint32 TriangleIndex = 0; TriangleIndex < NumTris; ++TriangleIndex)
            {
                FVertexInstanceID CornerInstanceIDs[3];

                for (int32 CornerIndex = 0; CornerIndex < 3; ++CornerIndex)
                {
                    const int32 VertexInstanceIndex = static_cast<int32>(TriangleIndex * 3 + CornerIndex);
                    const int32 VertexIndex = static_cast<int32>(Indexes[VertexInstanceIndex]);
                    const FVertexID CornerVertexID(VertexIndex);

                    const FVertexInstanceID AddedVertexInstanceID = OutReducedMesh.CreateVertexInstance(CornerVertexID);
                    CornerInstanceIDs[CornerIndex] = AddedVertexInstanceID;

                    OutVertexTangents[AddedVertexInstanceID] = Verts[VertexIndex].TangentX;
                    OutVertexBinormalSigns[AddedVertexInstanceID] = GetBasisDeterminantSign(
                        static_cast<FVector>(Verts[VertexIndex].TangentX),
                        static_cast<FVector>(Verts[VertexIndex].TangentY),
                        static_cast<FVector>(Verts[VertexIndex].TangentZ));
                    OutVertexNormals[AddedVertexInstanceID] = Verts[VertexIndex].TangentZ;
                    OutVertexColors[AddedVertexInstanceID] = Verts[VertexIndex].Color;

                    for (int32 TexCoordIndex = 0; TexCoordIndex < InMeshNumTexCoords; ++TexCoordIndex)
                    {
                        OutVertexUVs.Set(AddedVertexInstanceID, TexCoordIndex, Verts[VertexIndex].UVs[TexCoordIndex]);
                    }
                }

                const int32 MaterialIndex = MaterialIndexes[TriangleIndex];
                FPolygonGroupID MaterialPolygonGroupID(INDEX_NONE);
                if (const FPolygonGroupID* Existing = PolygonGroupMapping.Find(MaterialIndex))
                {
                    MaterialPolygonGroupID = *Existing;
                }
                else
                {
                    const FPolygonGroupID Candidate(MaterialIndex);
                    if (OutReducedMesh.PolygonGroups().IsValid(Candidate))
                    {
                        MaterialPolygonGroupID = Candidate;
                    }
                    else
                    {
                        MaterialPolygonGroupID = OutReducedMesh.CreatePolygonGroup();
                    }
                    PolygonGroupMapping.Add(MaterialIndex, MaterialPolygonGroupID);
                }

                TArray<FEdgeID> NewEdgeIDs;
                OutReducedMesh.CreateTriangle(MaterialPolygonGroupID, CornerInstanceIDs, &NewEdgeIDs);
            }

            TArray<FPolygonGroupID> EmptyGroups;
            for (const FPolygonGroupID PolygonGroupID : OutReducedMesh.PolygonGroups().GetElementIDs())
            {
                if (OutReducedMesh.GetPolygonGroupPolygonIDs(PolygonGroupID).Num() == 0)
                {
                    EmptyGroups.Add(PolygonGroupID);
                }
            }
            for (const FPolygonGroupID PolygonGroupID : EmptyGroups)
            {
                OutReducedMesh.DeletePolygonGroup(PolygonGroupID);
            }
        }

        virtual bool ReduceSkeletalMesh(USkeletalMesh* SkeletalMesh, int32 LODIndex, const ITargetPlatform* TargetPlatform) override
        {
            return false;
        }

        virtual bool IsSupported() const override
        {
            return true;
        }

        virtual bool IsReductionActive(const FMeshReductionSettings& ReductionSettings) const override
        {
            return IsReductionActive(ReductionSettings, 0, 0);
        }

        virtual bool IsReductionActive(const FMeshReductionSettings& ReductionSettings, uint32 NumVertices, uint32 NumTriangles) const override
        {
            const float ThresholdOne = (1.0f - UE_KINDA_SMALL_NUMBER);
            switch (ReductionSettings.TerminationCriterion)
            {
            case EStaticMeshReductionTerimationCriterion::Triangles:
                return ReductionSettings.PercentTriangles < ThresholdOne || ReductionSettings.MaxNumOfTriangles < NumTriangles;
            case EStaticMeshReductionTerimationCriterion::Vertices:
                return ReductionSettings.PercentVertices < ThresholdOne || ReductionSettings.MaxNumOfVerts < NumVertices;
            case EStaticMeshReductionTerimationCriterion::Any:
                return ReductionSettings.PercentTriangles < ThresholdOne ||
                       ReductionSettings.PercentVertices < ThresholdOne ||
                       ReductionSettings.MaxNumOfTriangles < NumTriangles ||
                       ReductionSettings.MaxNumOfVerts < NumVertices;
            default:
                return false;
            }
        }

        virtual bool IsReductionActive(const FSkeletalMeshOptimizationSettings& ReductionSettings) const override
        {
            return false;
        }

        virtual bool IsReductionActive(const FSkeletalMeshOptimizationSettings& ReductionSettings, uint32 NumVertices, uint32 NumTriangles) const override
        {
            return false;
        }
    };

    TUniquePtr<FQEMMeshReduction> GQEMMeshReduction;
}

void FQEMMeshReductionModule::StartupModule()
{
    GQEMMeshReduction = MakeUnique<FQEMMeshReduction>();

    FString LoadedDllPath;
    bQemApiReady = LoadQemRuntimeDll(LoadedDllPath);
    if (!bQemApiReady)
    {
        UE_LOG(LogQEMMeshReduction, Error, TEXT("Startup: failed to preload qem runtime DLL; simplification will be disabled."));
        IModularFeatures::Get().RegisterModularFeature(IMeshReductionModule::GetModularFeatureName(), this);
        return;
    }

    const uint32 AbiVersion = qem_get_abi_version();
    if (AbiVersion != QemExpectedAbiVersion)
    {
        UE_LOG(
            LogQEMMeshReduction,
            Error,
            TEXT("Unsupported qem ABI version. expected=%u, got=%u. Simplification disabled."),
            QemExpectedAbiVersion,
            AbiVersion);
        bQemApiReady = false;
        IModularFeatures::Get().RegisterModularFeature(IMeshReductionModule::GetModularFeatureName(), this);
        return;
    }

    GQemAbiContext = qem_context_create();
    UE_LOG(LogQEMMeshReduction, Log, TEXT("Startup complete. DLL=%s, ABI mode=v6, ABI version=%u, context=%p"), *LoadedDllPath, AbiVersion, GQemAbiContext);
    if (GQemAbiContext == nullptr)
    {
        UE_LOG(LogQEMMeshReduction, Warning, TEXT("Failed to create ABI v3 context at startup; will fallback per-call if needed."));
    }
    IModularFeatures::Get().RegisterModularFeature(IMeshReductionModule::GetModularFeatureName(), this);
}

void FQEMMeshReductionModule::ShutdownModule()
{
    UE_LOG(LogQEMMeshReduction, Log, TEXT("Shutdown begin."));
    IModularFeatures::Get().UnregisterModularFeature(IMeshReductionModule::GetModularFeatureName(), this);
    if (GQemAbiContext != nullptr)
    {
        qem_context_destroy(GQemAbiContext);
        GQemAbiContext = nullptr;
    }

    if (GQemRuntimeDllHandle != nullptr)
    {
        FPlatformProcess::FreeDllHandle(GQemRuntimeDllHandle);
        GQemRuntimeDllHandle = nullptr;
    }

    bQemApiReady = false;
    bLoggedApiUnavailable = false;
    GQEMMeshReduction.Reset();
    bLoggedFirstReduceCall = false;
    UE_LOG(LogQEMMeshReduction, Log, TEXT("Shutdown complete."));
}

IMeshReduction* FQEMMeshReductionModule::GetStaticMeshReductionInterface()
{
    return GQEMMeshReduction.Get();
}

IMeshReduction* FQEMMeshReductionModule::GetSkeletalMeshReductionInterface()
{
    return nullptr;
}

IMeshMerging* FQEMMeshReductionModule::GetMeshMergingInterface()
{
    return nullptr;
}

IMeshMerging* FQEMMeshReductionModule::GetDistributedMeshMergingInterface()
{
    return nullptr;
}

FString FQEMMeshReductionModule::GetName()
{
    return TEXT("QEMMeshReduction");
}
