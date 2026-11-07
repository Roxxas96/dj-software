# Test vector for the filled HTDemucs core (docs/stems-backend.md, "Verification"); the
# expected values are in crates/stems/src/model.rs.
# usage: python -I test_vector.py <955717e8.safetensors> <model.onnx>...
import sys,os,json,numpy as np,torch,onnxruntime as ort
sys.path.insert(0,os.path.dirname(os.path.abspath(__file__)))
from htdemucs_core import Core
from demucs.hf import load_safetensors_model
torch.backends.mha.set_fastpath_enabled(False)
L=343980
# LCG (PCG multiplier), planar order: channel 0 samples 0..L-1, then channel 1.
M=np.uint64(6364136223846793005); A=np.uint64(1442695040888963407)
s=np.uint64(0x853c49e6748fea9b); out=np.empty(2*L,dtype=np.float32)
with np.errstate(over='ignore'):
    for i in range(2*L):
        s=s*M+A; out[i]=np.float32((int(s)>>40)/16777216.0*2.0-1.0)*np.float32(0.5)
mix=out.reshape(1,2,L)
m=load_safetensors_model(sys.argv[1]).eval()
with torch.no_grad():
    mt=torch.from_numpy(mix); mag=m._magnitude(m._spec(mt)).numpy()
    ref=[t.numpy() for t in Core(m).eval()(mt,torch.from_numpy(mag))]
def stats(o): return {n:{'sum':float(o[i].astype(np.float64).sum()),'sumsq':float((o[i].astype(np.float64)**2).sum()),
    'per_source_sumsq':[float((o[i][0,s].astype(np.float64)**2).sum()) for s in range(4)],'first8':[float(v) for v in o[i].ravel()[:8]]} for i,n in enumerate(['spec_out','time_out'])}
res={'input':{'mix_first4':[float(v) for v in mix.ravel()[:4]],'mix_sum':float(mix.astype(np.float64).sum()),'mag_sum':float(mag.astype(np.float64).sum()),'mag_sumsq':float((mag.astype(np.float64)**2).sum())},'pytorch':stats(ref)}
for p in sys.argv[2:]:
    for prov in ['CPUExecutionProvider',('CoreMLExecutionProvider',{'ModelFormat':'MLProgram','MLComputeUnits':'ALL'})]:
        sess=ort.InferenceSession(p,providers=[prov]); o=sess.run(None,{'mix':mix,'mag':mag})
        agree=[round(float(10*np.log10((r.astype(np.float64)**2).sum()/((r.astype(np.float64)-x)**2).sum())),1) for r,x in zip(ref,o)]
        relmax=[float(np.abs(r-x).max()/np.abs(r).max()) for r,x in zip(ref,o)]
        res[os.path.basename(p)+':'+(prov if isinstance(prov,str) else prov[0])]={'agreement_dB(spec,time)':agree,'max_abs_err/max_abs':relmax,'sumsq':[float((x.astype(np.float64)**2).sum()) for x in o],'max_rel_err_per_source_sumsq':max(abs(float((x[0,s].astype(np.float64)**2).sum())/float((r[0,s].astype(np.float64)**2).sum())-1) for r,x in zip(ref,o) for s in range(4))}
print(json.dumps(res,indent=1))
